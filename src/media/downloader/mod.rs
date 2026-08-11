//! Secure HTTPS downloads with host, redirect, size, and disk safeguards.

mod http;
mod progress;
mod ssrf;
mod write;

#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use reqwest::{
    Client, Response,
    header::{ACCEPT, ACCEPT_ENCODING, ORIGIN, REFERER, USER_AGENT},
};
use tokio::time::timeout;
use tracing::Instrument;
use url::Url;

use crate::{
    error::{Error, Result},
    model::MediaSource,
    platforms,
};

use self::http::{
    check_response_status, checked_content_length, map_reqwest_download_error, pinned_http_client,
    reject_encoded_response, retry_transient_downloads,
};
use self::progress::ProgressReporter;
use self::ssrf::{normalize_allowed_hosts, resolve_public_addresses, validate_media_url};
use self::write::{
    WrittenMedia, create_private_file, effective_resume_offset, open_private_file_append,
    random_task_path, write_chunks,
};

const MAX_REDIRECTS: usize = 5;
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_MEDIA_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
pub(super) const DOWNLOAD_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_secs(1), Duration::from_secs(2)];

/// Request headers used to identify media downloads.
#[derive(Clone, Debug, Default)]
pub struct DownloadRequestIdentity {
    pub origin: Option<String>,
    pub referer: Option<String>,
    pub user_agent: Option<String>,
}

impl DownloadRequestIdentity {
    pub fn wechat_channels() -> Self {
        platforms::wechat::download_identity()
    }

    pub fn douyin() -> Self {
        platforms::douyin::download_identity()
    }

    pub fn bilibili() -> Self {
        platforms::bilibili::download_identity()
    }
}

/// Cumulative progress reported at fixed thresholds when the length is known.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub percent: u8,
}

pub(super) type ProgressCallback = Arc<dyn Fn(DownloadProgress) + Send + Sync + 'static>;

/// A completed download stored on disk.
///
/// The file is deleted on drop unless retained with [`Self::into_path`] or
/// [`Self::keep`]. Call [`Self::cleanup`] to remove it early.
#[derive(Debug)]
#[must_use = "downloaded media must be uploaded, kept, or explicitly cleaned up"]
pub struct DownloadedMedia {
    pub path: PathBuf,
    pub bytes: u64,
    armed: bool,
}

impl DownloadedMedia {
    pub(super) fn new(path: PathBuf, bytes: u64) -> Self {
        Self {
            path,
            bytes,
            armed: true,
        }
    }

    pub async fn cleanup(&self) -> Result<()> {
        match tokio::fs::remove_file(&self.path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Error::Io(error)),
        }
    }

    /// Keeps the file and transfers ownership of its path to the caller.
    pub fn into_path(mut self) -> PathBuf {
        self.armed = false;
        std::mem::take(&mut self.path)
    }

    /// Keeps the file and returns its path.
    pub fn keep(self) -> PathBuf {
        self.into_path()
    }
}

impl Drop for DownloadedMedia {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Downloads media over HTTPS under explicit network and storage limits.
#[derive(Clone, Debug)]
pub struct MediaDownloader {
    workspace_dir: Arc<PathBuf>,
    allowed_hosts: Arc<std::collections::HashSet<String>>,
    request_timeout: Duration,
    request_identity: DownloadRequestIdentity,
    disk_write_budget: Arc<StdMutex<DiskWriteBudget>>,
}

#[derive(Debug, Default)]
pub(super) struct DiskWriteBudget {
    pub(super) unchecked_bytes: u64,
}

impl MediaDownloader {
    /// Creates a downloader with an explicit host allowlist.
    pub fn with_allowed_hosts(
        workspace_dir: impl Into<PathBuf>,
        allowed_hosts: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Self> {
        Self::with_options(
            workspace_dir,
            allowed_hosts,
            REQUEST_TIMEOUT,
            DownloadRequestIdentity::default(),
        )
    }

    pub fn for_wechat_channels(workspace_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::with_options(
            workspace_dir,
            platforms::wechat::REVIEWED_MEDIA_HOSTS.iter().copied(),
            REQUEST_TIMEOUT,
            platforms::wechat::download_identity(),
        )
    }

    pub fn for_douyin(workspace_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::with_options(
            workspace_dir,
            platforms::douyin::REVIEWED_MEDIA_HOSTS.iter().copied(),
            REQUEST_TIMEOUT,
            platforms::douyin::download_identity(),
        )
    }

    pub fn for_bilibili(workspace_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::with_options(
            workspace_dir,
            platforms::bilibili::REVIEWED_MEDIA_HOSTS.iter().copied(),
            REQUEST_TIMEOUT,
            platforms::bilibili::download_identity(),
        )
    }

    pub fn for_platform(
        platform_id: crate::PlatformId,
        workspace_dir: impl Into<PathBuf>,
    ) -> Result<Self> {
        match platform_id {
            crate::PlatformId::WechatChannels => Self::for_wechat_channels(workspace_dir),
            crate::PlatformId::Douyin => Self::for_douyin(workspace_dir),
            crate::PlatformId::Bilibili => Self::for_bilibili(workspace_dir),
        }
    }

    pub fn with_options(
        workspace_dir: impl Into<PathBuf>,
        allowed_hosts: impl IntoIterator<Item = impl AsRef<str>>,
        request_timeout: Duration,
        request_identity: DownloadRequestIdentity,
    ) -> Result<Self> {
        if request_timeout.is_zero() {
            return Err(Error::Config("媒体下载超时必须大于零".to_owned()));
        }
        let allowed_hosts = normalize_allowed_hosts(allowed_hosts)?;

        Ok(Self {
            workspace_dir: Arc::new(workspace_dir.into()),
            allowed_hosts: Arc::new(allowed_hosts),
            request_timeout,
            request_identity,
            disk_write_budget: Arc::new(StdMutex::new(DiskWriteBudget::default())),
        })
    }

    pub fn allowed_hosts(&self) -> &std::collections::HashSet<String> {
        self.allowed_hosts.as_ref()
    }

    /// Clones this downloader with a tighter request timeout.
    pub fn with_timeout(&self, request_timeout: Duration) -> Result<Self> {
        if request_timeout.is_zero() {
            return Err(Error::Config("媒体下载超时必须大于零".to_owned()));
        }
        Ok(Self {
            workspace_dir: Arc::clone(&self.workspace_dir),
            allowed_hosts: Arc::clone(&self.allowed_hosts),
            request_timeout: self.request_timeout.min(request_timeout),
            request_identity: self.request_identity.clone(),
            disk_write_budget: Arc::clone(&self.disk_write_budget),
        })
    }

    pub async fn download(&self, source: &MediaSource) -> Result<DownloadedMedia> {
        self.download_source_with_callback(source, None).await
    }

    pub async fn download_url(&self, url: &Url) -> Result<DownloadedMedia> {
        self.download_url_with_callback(url, None, None, None).await
    }

    /// Downloads each source to a separate file.
    ///
    /// Completed files are removed if a later download fails.
    pub async fn download_all<'a, I>(&self, sources: I) -> Result<Vec<DownloadedMedia>>
    where
        I: IntoIterator<Item = &'a MediaSource>,
    {
        let span = tracing::info_span!("media.download_all");
        async move {
            let mut saved = Vec::new();
            for source in sources {
                match self.download(source).await {
                    Ok(media) => saved.push(media),
                    Err(error) => {
                        for media in saved.drain(..) {
                            let _ = media.cleanup().await;
                        }
                        return Err(error);
                    }
                }
            }
            if saved.is_empty() {
                return Err(Error::MediaUnavailable);
            }
            Ok(saved)
        }
        .instrument(span)
        .await
    }

    /// Tries sources in order until one yields playable media.
    ///
    /// Sources with a `decode_key` are decrypted while written and validated as BMFF.
    pub async fn download_playable<'a, I>(&self, sources: I) -> Result<DownloadedMedia>
    where
        I: IntoIterator<Item = &'a MediaSource>,
    {
        let span = tracing::info_span!("media.download_playable");
        async move {
            let mut last_error = None;
            for source in sources {
                let host = source.url.host_str().unwrap_or("-");
                let attempt = tracing::info_span!(
                    "media.download_source",
                    host,
                    has_decode_key = source.decode_key.is_some()
                );
                match self
                    .download_source_with_callback(source, None)
                    .instrument(attempt)
                    .await
                {
                    Ok(media) => {
                        if source.decode_key.is_some() {
                            match crate::media::file_prefix_looks_like_bmff(&media.path).await {
                                Ok(true) => return Ok(media),
                                Ok(false) => {
                                    let _ = media.cleanup().await;
                                    last_error = Some(Error::InvalidMedia(
                                        "decodeKey 与媒体不匹配，解密后没有有效 BMFF 文件头".into(),
                                    ));
                                }
                                Err(error) => {
                                    let _ = media.cleanup().await;
                                    last_error = Some(error);
                                }
                            }
                        } else {
                            return Ok(media);
                        }
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            Err(last_error.unwrap_or(Error::MediaUnavailable))
        }
        .instrument(span)
        .await
    }

    /// Downloads one source and reports known-length progress at 20% intervals.
    ///
    /// The synchronous callback should return promptly.
    pub async fn download_with_progress<F>(
        &self,
        source: &MediaSource,
        on_progress: F,
    ) -> Result<DownloadedMedia>
    where
        F: Fn(DownloadProgress) + Send + Sync + 'static,
    {
        self.download_source_with_callback(source, Some(Arc::new(on_progress)))
            .await
    }

    async fn download_source_with_callback(
        &self,
        source: &MediaSource,
        progress_callback: Option<ProgressCallback>,
    ) -> Result<DownloadedMedia> {
        self.download_url_with_callback(
            &source.url,
            source.size_hint,
            progress_callback,
            source.decode_key,
        )
        .await
    }

    async fn download_url_with_callback(
        &self,
        url: &Url,
        size_hint: Option<u64>,
        progress_callback: Option<ProgressCallback>,
        decode_key: Option<u64>,
    ) -> Result<DownloadedMedia> {
        // Reuse one path across retries so unencrypted downloads can resume.
        // Encrypted prefixes must restart from byte zero.
        tokio::fs::create_dir_all(self.workspace_dir.as_path())
            .await
            .map_err(|_| Error::Storage(self.workspace_dir.as_ref().clone()))?;
        let path = random_task_path(self.workspace_dir.as_path(), url);
        let path = Arc::new(path);
        let allow_resume = decode_key.is_none();

        let result = timeout(
            self.request_timeout,
            retry_transient_downloads(
                || {
                    let path = Arc::clone(&path);
                    let progress_callback = progress_callback.clone();
                    async move {
                        let resume_from = if allow_resume {
                            tokio::fs::metadata(path.as_path())
                                .await
                                .map(|meta| meta.len())
                                .unwrap_or(0)
                        } else {
                            if path.exists() {
                                let _ = tokio::fs::remove_file(path.as_path()).await;
                            }
                            0
                        };
                        self.download_url_within_deadline(
                            url,
                            size_hint,
                            progress_callback,
                            decode_key,
                            path.as_path().to_path_buf(),
                            resume_from,
                        )
                        .await
                    }
                },
                &DOWNLOAD_RETRY_DELAYS,
            ),
        )
        .await
        .map_err(|_| Error::Download("媒体下载总超时".to_owned()))?;

        if result.is_err() {
            let _ = tokio::fs::remove_file(path.as_path()).await;
        }
        result
    }

    async fn download_url_within_deadline(
        &self,
        url: &Url,
        _size_hint: Option<u64>,
        progress_callback: Option<ProgressCallback>,
        decode_key: Option<u64>,
        path: PathBuf,
        mut resume_from: u64,
    ) -> Result<DownloadedMedia> {
        // Allow one full restart when a server ignores `Range`.
        for _ in 0..2 {
            let response = self.follow_redirects(url.clone(), resume_from).await?;
            let status = response.status();
            let Some(next_resume) = effective_resume_offset(resume_from, status) else {
                // A status without resume semantics is handled as an error.
                return check_response_status(status)
                    .map(|()| unreachable!("successful status should yield a resume offset"));
            };
            if resume_from > 0 && next_resume == 0 {
                // The server ignored `Range`; discard the partial file and retry once.
                let _ = tokio::fs::remove_file(&path).await;
                resume_from = 0;
                continue;
            }
            resume_from = next_resume;
            // Resumed requests accept 206; fresh requests still require 200.
            if !(resume_from > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT) {
                check_response_status(status)?;
            }
            reject_encoded_response(&response)?;

            let content_length = checked_content_length(&response)?;
            let total_hint = if resume_from > 0 {
                content_length.map(|len| len.saturating_add(resume_from))
            } else {
                content_length
            };

            return self
                .stream_response(
                    response,
                    total_hint,
                    progress_callback,
                    decode_key,
                    path,
                    resume_from,
                )
                .await;
        }
        Err(Error::Download("媒体下载重试逻辑失败".into()))
    }

    async fn follow_redirects(&self, initial_url: Url, resume_from: u64) -> Result<Response> {
        let mut current = initial_url;
        let mut pinned_clients = HashMap::<String, Client>::new();

        for redirect_count in 0..=MAX_REDIRECTS {
            let host = validate_media_url(&current, &self.allowed_hosts)?.to_ascii_lowercase();
            if !pinned_clients.contains_key(&host) {
                let addresses = resolve_public_addresses(&host).await?;
                let client = pinned_http_client(&host, &addresses, self.request_timeout)?;
                pinned_clients.insert(host.clone(), client);
            }
            let client = pinned_clients
                .get(&host)
                .ok_or_else(|| Error::Download("媒体 HTTP 客户端初始化失败".to_owned()))?;

            let response = self
                .request_with_client(client, &current, resume_from)
                .await?;

            if !response.status().is_redirection() {
                return Ok(response);
            }

            if redirect_count == MAX_REDIRECTS {
                return Err(Error::Download("媒体重定向次数过多".to_owned()));
            }

            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or_else(|| Error::Download("媒体重定向缺少 Location".to_owned()))?
                .to_str()
                .map_err(|_| Error::Download("媒体重定向地址无效".to_owned()))?;

            current = current
                .join(location)
                .map_err(|_| Error::Download("媒体重定向地址无效".to_owned()))?;
        }

        Err(Error::Download("媒体重定向次数过多".to_owned()))
    }

    async fn request_with_client(
        &self,
        client: &Client,
        url: &Url,
        resume_from: u64,
    ) -> Result<Response> {
        let mut request = client
            .get(url.clone())
            .header(ACCEPT, "*/*")
            .header(ACCEPT_ENCODING, "identity");
        if resume_from > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={resume_from}-"));
        }
        if let Some(origin) = self.request_identity.origin.as_deref() {
            request = request.header(ORIGIN, origin);
        }
        if let Some(referer) = self.request_identity.referer.as_deref() {
            request = request.header(REFERER, referer);
        }
        let user_agent = self
            .request_identity
            .user_agent
            .as_deref()
            .unwrap_or(DEFAULT_MEDIA_USER_AGENT);
        request = request.header(USER_AGENT, user_agent);
        request.send().await.map_err(map_reqwest_download_error)
    }

    async fn stream_response(
        &self,
        mut response: Response,
        content_length: Option<u64>,
        progress_callback: Option<ProgressCallback>,
        decode_key: Option<u64>,
        path: PathBuf,
        resume_from: u64,
    ) -> Result<DownloadedMedia> {
        let pending_file = if resume_from > 0 && path.exists() {
            open_private_file_append(path.clone()).await?
        } else {
            let _ = tokio::fs::remove_file(&path).await;
            create_private_file(path.clone()).await?
        };
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let disk_write_budget = Arc::clone(&self.disk_write_budget);
        let (progress_reporter, _progress_guard) =
            ProgressReporter::new(content_length, progress_callback);
        // Decryption starts at byte zero, so resumed writes never initialize XOR state.
        let prefix_xor = if resume_from == 0 {
            decode_key.map(crate::platforms::wechat::PrefixXor::new)
        } else {
            None
        };
        let initial_bytes = resume_from;

        let writer = tokio::task::spawn_blocking(move || -> Result<WrittenMedia> {
            write_chunks(
                pending_file,
                &mut receiver,
                progress_reporter,
                disk_write_budget,
                prefix_xor,
                initial_bytes,
            )
        });

        let mut streamed_bytes = resume_from;
        let stream_result: Result<()> = async {
            loop {
                let chunk = timeout(DOWNLOAD_IDLE_TIMEOUT, response.chunk())
                    .await
                    .map_err(|_| Error::Network("媒体响应读取超时".to_owned()))?
                    .map_err(map_reqwest_download_error)?;
                let Some(chunk) = chunk else {
                    break;
                };
                streamed_bytes = streamed_bytes
                    .checked_add(chunk.len() as u64)
                    .ok_or_else(|| Error::Download("媒体大小计算溢出".into()))?;

                sender
                    .send(chunk)
                    .await
                    .map_err(|_| Error::Download("媒体文件写入失败".to_owned()))?;
            }
            Ok(())
        }
        .await;

        drop(sender);
        let writer_result = match writer.await {
            Ok(result) => result,
            Err(_) => {
                let _ = tokio::fs::remove_file(&path).await;
                return Err(Error::Download("媒体文件写入任务失败".to_owned()));
            }
        };

        let outcome = match stream_result.and(writer_result) {
            Ok(outcome) => outcome,
            Err(error) => return Err(error),
        };

        if outcome.media.bytes != streamed_bytes {
            return Err(Error::Download("媒体文件写入不完整".to_owned()));
        }
        if resume_from > 0 {
            tracing::debug!(
                event = "media_download_resumed",
                resume_from,
                total = streamed_bytes,
                "resumed partial media download"
            );
        }

        let disk_bytes = match tokio::fs::metadata(&outcome.media.path).await {
            Ok(metadata) => metadata.len(),
            Err(_) => return Err(Error::Storage(self.workspace_dir.as_ref().clone())),
        };
        if disk_bytes != outcome.media.bytes {
            return Err(Error::Download("媒体文件落盘大小不一致".to_owned()));
        }

        if let Some(expected) = content_length
            && disk_bytes != expected
        {
            return Err(Error::Network(
                "媒体文件大小与 Content-Length 不一致".to_owned(),
            ));
        }

        let WrittenMedia {
            media,
            mut progress_reporter,
        } = outcome;
        if let Some(reporter) = &mut progress_reporter {
            reporter.report_complete(media.bytes);
        }

        Ok(media)
    }
}
