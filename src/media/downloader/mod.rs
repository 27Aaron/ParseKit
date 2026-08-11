//! Secure HTTPS downloads with host, redirect, size, and disk safeguards.

mod http;
mod progress;
mod size_probe;
mod ssrf;
mod write;

#[cfg(test)]
mod tests;

pub use size_probe::enrich_missing_size_hints;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use reqwest::{
    Client, Response,
    header::{ACCEPT, ACCEPT_ENCODING, HeaderValue, ORIGIN, REFERER, USER_AGENT},
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
    check_response_status, checked_content_length, map_reqwest_download_error, parse_content_range,
    pinned_http_client, reject_encoded_response, retry_transient_downloads,
};
use self::progress::ProgressReporter;
use self::ssrf::{normalize_allowed_hosts, resolve_public_addresses, validate_media_url};
use self::write::{
    WrittenMedia, create_private_file, effective_resume_offset, existing_complete_download,
    extension_from_content_type, extension_from_url, media_task_path, open_private_file_append,
    path_with_better_extension, safe_file_stem, write_chunks,
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
    pub fn for_platform(platform_id: crate::PlatformId) -> Self {
        platforms::platform_spec(platform_id).download_identity()
    }

    pub fn wechat() -> Self {
        Self::for_platform(crate::PlatformId::Wechat)
    }

    pub fn douyin() -> Self {
        Self::for_platform(crate::PlatformId::Douyin)
    }

    pub fn bilibili() -> Self {
        Self::for_platform(crate::PlatformId::Bilibili)
    }
}

/// Cumulative whole-percent download progress.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub percent: u8,
}

pub(super) type ProgressCallback = Arc<dyn Fn(DownloadProgress) + Send + Sync + 'static>;

/// Downloaded file that removes new data on drop unless explicitly kept.
#[derive(Debug)]
#[must_use = "downloaded media must be uploaded, kept, or explicitly cleaned up"]
pub struct DownloadedMedia {
    pub path: PathBuf,
    pub bytes: u64,
    /// Whether an existing file was reused and retained on drop.
    pub skipped: bool,
    armed: bool,
}

impl DownloadedMedia {
    pub(super) fn new(path: PathBuf, bytes: u64) -> Self {
        Self {
            path,
            bytes,
            skipped: false,
            armed: true,
        }
    }

    /// Wraps an existing file without taking cleanup ownership.
    pub(super) fn reused(path: PathBuf, bytes: u64) -> Self {
        Self {
            path,
            bytes,
            skipped: true,
            armed: false,
        }
    }

    pub async fn cleanup(&self) -> Result<()> {
        if self.skipped {
            return Ok(());
        }
        match tokio::fs::remove_file(&self.path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Error::Io(error)),
        }
    }

    /// Retains the file and returns its path.
    pub fn into_path(mut self) -> PathBuf {
        self.armed = false;
        std::mem::take(&mut self.path)
    }

    /// Alias for [`Self::into_path`].
    pub fn keep(self) -> PathBuf {
        self.into_path()
    }

    async fn relocate(mut self, target: PathBuf) -> Result<Self> {
        if self.path == target {
            return Ok(self);
        }
        if tokio::fs::rename(&self.path, &target).await.is_err() {
            #[cfg(not(windows))]
            return Err(Error::Storage(target));

            #[cfg(windows)]
            {
                // Windows rename cannot replace a file; remove only a verified target.
                match tokio::fs::symlink_metadata(&target).await {
                    Ok(metadata) if metadata.is_file() => {}
                    Ok(_) => return Err(Error::Storage(target)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Err(Error::Storage(target));
                    }
                    Err(_) => return Err(Error::Storage(target)),
                }
                tokio::fs::remove_file(&target)
                    .await
                    .map_err(|_| Error::Storage(target.clone()))?;
                tokio::fs::rename(&self.path, &target)
                    .await
                    .map_err(|_| Error::Storage(target.clone()))?;
            }
        }
        self.path = target;
        Ok(self)
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
    file_stem: Option<Arc<str>>,
    skip_existing: bool,
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

    pub fn for_wechat(workspace_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::for_platform(crate::PlatformId::Wechat, workspace_dir)
    }

    pub fn for_douyin(workspace_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::for_platform(crate::PlatformId::Douyin, workspace_dir)
    }

    pub fn for_bilibili(workspace_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::for_platform(crate::PlatformId::Bilibili, workspace_dir)
    }

    pub fn for_platform(
        platform_id: crate::PlatformId,
        workspace_dir: impl Into<PathBuf>,
    ) -> Result<Self> {
        let spec = platforms::platform_spec(platform_id);
        Self::with_options(
            workspace_dir,
            spec.reviewed_media_hosts().iter().copied(),
            REQUEST_TIMEOUT,
            spec.download_identity(),
        )
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
        validate_request_identity(&request_identity)?;

        Ok(Self {
            workspace_dir: Arc::new(workspace_dir.into()),
            allowed_hosts: Arc::new(allowed_hosts),
            request_timeout,
            request_identity,
            disk_write_budget: Arc::new(StdMutex::new(DiskWriteBudget::default())),
            file_stem: None,
            skip_existing: true,
        })
    }

    pub fn allowed_hosts(&self) -> &std::collections::HashSet<String> {
        self.allowed_hosts.as_ref()
    }

    pub fn with_file_stem(mut self, file_stem: impl Into<String>) -> Self {
        let stem = file_stem.into();
        self.file_stem = safe_file_stem(&stem).map(Arc::from);
        self
    }

    pub fn file_stem(&self) -> Option<&str> {
        self.file_stem.as_deref()
    }

    /// Controls complete-file reuse; enabled by default.
    pub fn with_skip_existing(mut self, skip_existing: bool) -> Self {
        self.skip_existing = skip_existing;
        self
    }

    pub fn skip_existing(&self) -> bool {
        self.skip_existing
    }

    /// Clones the downloader with a timeout no greater than the current limit.
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
            file_stem: self.file_stem.clone(),
            skip_existing: self.skip_existing,
        })
    }

    pub async fn download(&self, source: &MediaSource) -> Result<DownloadedMedia> {
        self.download_indexed(source, 0).await
    }

    /// Downloads one source with a multi-file sequence suffix.
    pub async fn download_indexed(
        &self,
        source: &MediaSource,
        sequence: u32,
    ) -> Result<DownloadedMedia> {
        self.download_source_with_callback(source, None, sequence)
            .await
    }

    pub async fn download_url(&self, url: &Url) -> Result<DownloadedMedia> {
        self.download_url_with_callback(url, None, None, None, 0)
            .await
    }

    /// Downloads each source separately and rolls back new files on failure.
    pub async fn download_all<'a, I>(&self, sources: I) -> Result<Vec<DownloadedMedia>>
    where
        I: IntoIterator<Item = &'a MediaSource>,
    {
        let span = tracing::info_span!("media.download_all");
        async move {
            let mut saved = Vec::new();
            for (index, source) in sources.into_iter().enumerate() {
                let sequence = u32::try_from(index)
                    .map_err(|_| Error::Config("媒体源数量超出支持范围".into()))?;
                match self
                    .download_source_with_callback(source, None, sequence)
                    .await
                {
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

    /// Tries sources in order, decrypting and validating keyed BMFF media.
    pub async fn download_playable<'a, I>(&self, sources: I) -> Result<DownloadedMedia>
    where
        I: IntoIterator<Item = &'a MediaSource>,
    {
        self.download_playable_with_progress(sources, |_| {}).await
    }

    /// Adds whole-percent progress to [`Self::download_playable`].
    ///
    /// The synchronous callback should return promptly.
    pub async fn download_playable_with_progress<'a, I, F>(
        &self,
        sources: I,
        on_progress: F,
    ) -> Result<DownloadedMedia>
    where
        I: IntoIterator<Item = &'a MediaSource>,
        F: Fn(DownloadProgress) + Send + Sync + 'static,
    {
        let callback: ProgressCallback = Arc::new(on_progress);
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
                    .download_source_with_callback(source, Some(Arc::clone(&callback)), 0)
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

    /// Downloads one source with whole-percent progress when length is known.
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
        self.download_indexed_with_progress(source, 0, on_progress)
            .await
    }

    /// Adds a sequence suffix to [`Self::download_with_progress`].
    pub async fn download_indexed_with_progress<F>(
        &self,
        source: &MediaSource,
        sequence: u32,
        on_progress: F,
    ) -> Result<DownloadedMedia>
    where
        F: Fn(DownloadProgress) + Send + Sync + 'static,
    {
        self.download_source_with_callback(source, Some(Arc::new(on_progress)), sequence)
            .await
    }

    async fn download_source_with_callback(
        &self,
        source: &MediaSource,
        progress_callback: Option<ProgressCallback>,
        sequence: u32,
    ) -> Result<DownloadedMedia> {
        self.download_url_with_callback(
            &source.url,
            source.size_hint,
            progress_callback,
            source.decode_key,
            sequence,
        )
        .await
    }

    async fn download_url_with_callback(
        &self,
        url: &Url,
        size_hint: Option<u64>,
        progress_callback: Option<ProgressCallback>,
        decode_key: Option<u64>,
        sequence: u32,
    ) -> Result<DownloadedMedia> {
        // Stable paths enable resume; encrypted prefixes always restart.
        let preferred_ext = extension_from_url(url);
        if self.skip_existing
            && let Some((existing, bytes)) = existing_complete_download(
                self.workspace_dir.as_path(),
                self.file_stem.as_deref(),
                sequence,
                preferred_ext,
                size_hint,
            )
            .await
        {
            tracing::info!(
                path = %existing.display(),
                bytes,
                "reusing complete local media file"
            );
            return Ok(DownloadedMedia::reused(existing, bytes));
        }

        let path = media_task_path(
            self.workspace_dir.as_path(),
            url,
            self.file_stem.as_deref(),
            sequence,
        );
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
                            tokio::fs::symlink_metadata(path.as_path())
                                .await
                                .ok()
                                .filter(|meta| meta.is_file() && !meta.file_type().is_symlink())
                                .map_or(0, |meta| meta.len())
                        } else {
                            let _ = tokio::fs::remove_file(path.as_path()).await;
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
        // Permit one clean restart when resume is unsupported.
        for _ in 0..2 {
            let response = self.follow_redirects(url.clone(), resume_from).await?;
            let status = response.status();
            if resume_from > 0 && status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                // A 416 makes the local partial untrustworthy.
                let _ = tokio::fs::remove_file(&path).await;
                resume_from = 0;
                continue;
            }
            let Some(next_resume) = effective_resume_offset(resume_from, status) else {
                return check_response_status(status)
                    .map(|()| unreachable!("successful status should yield a resume offset"));
            };
            if resume_from > 0 && next_resume == 0 {
                // The server ignored `Range`; retry without the partial file.
                let _ = tokio::fs::remove_file(&path).await;
                resume_from = 0;
                continue;
            }
            resume_from = next_resume;
            if !(resume_from > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT) {
                check_response_status(status)?;
            }
            reject_encoded_response(&response)?;

            // Rename extension-less files only after a complete transfer.
            let mut completed_path = path.clone();
            if let Some(ext) = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(extension_from_content_type)
            {
                completed_path = path_with_better_extension(completed_path, ext);
            }
            // Keyed WeChat media is BMFF video.
            if decode_key.is_some() {
                completed_path = path_with_better_extension(completed_path, "mp4");
            }

            let content_length = checked_content_length(&response)?;
            let (total_hint, ranged_response_length) = if resume_from > 0 {
                let range = parse_content_range(response.headers());
                let valid_range = range.is_some_and(|range| {
                    range.start == Some(resume_from)
                        && range.total.is_some()
                        && content_length
                            .is_none_or(|length| range.response_length() == Some(length))
                });
                if !valid_range {
                    // Never append a non-conforming range response.
                    let _ = tokio::fs::remove_file(&path).await;
                    resume_from = 0;
                    continue;
                }
                let range = range.expect("validated range is present");
                (range.total, range.response_length())
            } else {
                (content_length, None)
            };
            if total_hint == Some(0) {
                return Err(Error::InvalidMedia("媒体响应为空".to_owned()));
            }

            let media = self
                .stream_response(
                    response,
                    (total_hint, ranged_response_length),
                    progress_callback,
                    decode_key,
                    path,
                    resume_from,
                )
                .await?;
            return media.relocate(completed_path).await;
        }
        Err(Error::Download("媒体下载重试逻辑失败".into()))
    }

    async fn follow_redirects(&self, initial_url: Url, resume_from: u64) -> Result<Response> {
        let mut current = initial_url;
        let mut pinned_clients = HashMap::<String, Client>::new();

        for redirect_count in 0..=MAX_REDIRECTS {
            let host = validate_media_url(&current, &self.allowed_hosts)?.to_ascii_lowercase();
            let port = current.port_or_known_default().unwrap_or(443);
            // Pin each host and port independently.
            let client_key = format!("{host}:{port}");
            if !pinned_clients.contains_key(&client_key) {
                let addresses = resolve_public_addresses(&host, port).await?;
                let client = pinned_http_client(&host, &addresses, self.request_timeout)?;
                pinned_clients.insert(client_key.clone(), client);
            }
            let client = pinned_clients
                .get(&client_key)
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
        length_hints: (Option<u64>, Option<u64>),
        progress_callback: Option<ProgressCallback>,
        decode_key: Option<u64>,
        path: PathBuf,
        resume_from: u64,
    ) -> Result<DownloadedMedia> {
        let (content_length, ranged_response_length) = length_hints;
        // Delay filesystem changes until the response is validated.
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|_| Error::Storage(parent.to_path_buf()))?;
        }
        let pending_file = if resume_from > 0 {
            open_private_file_append(path.clone(), resume_from).await?
        } else {
            let _ = tokio::fs::remove_file(&path).await;
            create_private_file(path.clone()).await?
        };
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let disk_write_budget = Arc::clone(&self.disk_write_budget);
        let (progress_reporter, _progress_guard) =
            ProgressReporter::new(content_length, progress_callback, resume_from);
        // Prefix decryption is not resumable.
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

        // Prefer the writer's concrete storage error.
        let outcome = writer_result?;
        if let Err(error) = stream_result {
            if decode_key.is_none() && outcome.media.bytes > 0 && matches!(error, Error::Network(_))
            {
                // Retain an unencrypted prefix for the next retry.
                let _ = outcome.media.into_path();
            }
            return Err(error);
        }

        if outcome.media.bytes != streamed_bytes {
            return Err(Error::Download("媒体文件写入不完整".to_owned()));
        }
        if let Some(expected) = ranged_response_length {
            let transferred = streamed_bytes.saturating_sub(resume_from);
            if transferred != expected {
                if decode_key.is_none() && transferred < expected {
                    let _ = outcome.media.into_path();
                }
                return Err(Error::Network(
                    "媒体分段响应大小与 Content-Range 不一致".to_owned(),
                ));
            }
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

        if disk_bytes == 0 {
            return Err(Error::InvalidMedia("媒体响应为空".to_owned()));
        }

        if let Some(expected) = content_length
            && disk_bytes != expected
        {
            if decode_key.is_none() && disk_bytes < expected {
                let _ = outcome.media.into_path();
            }
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

fn validate_request_identity(identity: &DownloadRequestIdentity) -> Result<()> {
    for value in [
        identity.origin.as_deref(),
        identity.referer.as_deref(),
        identity.user_agent.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        HeaderValue::try_from(value)
            .map_err(|_| Error::Config("媒体请求标识包含无效 HTTP 头值".to_owned()))?;
    }
    Ok(())
}
