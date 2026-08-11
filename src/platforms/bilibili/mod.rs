//! Resolve public Bilibili videos from BV, av, and b23 links.

mod parse;
mod share;

#[cfg(test)]
mod tests;

use std::time::Duration;

use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, ACCEPT_LANGUAGE, LOCATION, REFERER, USER_AGENT},
    redirect::Policy,
};
use serde_json::Value;
use url::Url;

use crate::{
    Error, PlatformId, Result,
    media::DownloadRequestIdentity,
    model::{MediaSource, ResolvedPost},
    platforms::{
        PlatformResolver,
        util::{map_network_error, read_body_limited},
    },
    url::{CleanPolicy, clean_tracking_params},
};

use self::{
    parse::{build_post_from_view, collect_play_sources},
    share::{VideoId, is_b23_host, parse_video_id},
};

pub use self::share::extract_share_url;

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_SHORTLINK_REDIRECTS: usize = 8;
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const VIEW_API: &str = "https://api.bilibili.com/x/web-interface/view";
const PLAYURL_API: &str = "https://api.bilibili.com/x/player/playurl";

/// Reviewed media hosts; a leading dot matches subdomains only.
pub const REVIEWED_MEDIA_HOSTS: &[&str] = &[
    ".bilivideo.com",
    ".bilivideo.cn",
    ".akamaized.net",
    ".hdslb.com",
    "upos-sz-mirrorcos.bilivideo.com",
    "upos-sz-mirrorhw.bilivideo.com",
    "upos-sz-mirrorali.bilivideo.com",
    "upos-sz-estgcos.bilivideo.com",
    "upos-hz-mirrorakam.akamaized.net",
];

pub const REVIEWED_BILIBILI_MEDIA_HOSTS: &[&str] = REVIEWED_MEDIA_HOSTS;

pub fn download_identity() -> DownloadRequestIdentity {
    DownloadRequestIdentity {
        origin: Some("https://www.bilibili.com".into()),
        referer: Some("https://www.bilibili.com/".into()),
        user_agent: Some(USER_AGENT_VALUE.into()),
    }
}

#[derive(Clone)]
pub struct BilibiliResolver {
    client: Client,
    timeout: Duration,
}

impl std::fmt::Debug for BilibiliResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BilibiliResolver")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl BilibiliResolver {
    pub fn new() -> Result<Self> {
        let timeout = RESOLVE_TIMEOUT;
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .redirect(Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| Error::Config("无法初始化哔哩哔哩 HTTP 客户端".into()))?;
        Ok(Self { client, timeout })
    }

    pub async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        let url = extract_share_url(input)?;
        self.resolve_share_url(&url).await
    }

    pub async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        let url = extract_share_url(url.as_str())?;
        self.resolve_share_url(&url).await
    }

    async fn resolve_share_url(&self, url: &Url) -> Result<ResolvedPost> {
        tokio::time::timeout(self.timeout, self.resolve_normalized(url))
            .await
            .map_err(|_| Error::Network("哔哩哔哩解析总超时".into()))?
    }

    async fn resolve_normalized(&self, url: &Url) -> Result<ResolvedPost> {
        let mut current = clean_tracking_params(url, CleanPolicy::SHARE_PAGE);
        if is_b23_host(&current) {
            current = self.expand_short_link(current).await?;
        }
        let id = parse_video_id(&current)?;
        let view = self.request_view(&id).await?;
        build_post_from_view(&id, &view, self).await
    }

    async fn expand_short_link(&self, mut current: Url) -> Result<Url> {
        for _ in 0..MAX_SHORTLINK_REDIRECTS {
            if !is_b23_host(&current) {
                return Ok(current);
            }
            if current.scheme() != "https" {
                return Err(Error::UpstreamChanged);
            }
            let response = self
                .client
                .get(current.clone())
                .header(USER_AGENT, USER_AGENT_VALUE)
                .header(ACCEPT, "*/*")
                .send()
                .await
                .map_err(|error| map_network_error(&error, "短链请求超时", "无法展开 b23 短链"))?;
            if !response.status().is_redirection() {
                return Err(Error::UpstreamChanged);
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(Error::UpstreamChanged)?;
            current = current.join(location).map_err(|_| Error::UpstreamChanged)?;
            current = clean_tracking_params(&current, CleanPolicy::SHARE_PAGE);
        }
        Err(Error::UpstreamChanged)
    }

    async fn request_view(&self, id: &VideoId) -> Result<Value> {
        let mut endpoint =
            Url::parse(VIEW_API).map_err(|_| Error::Config("哔哩哔哩 view API 地址无效".into()))?;
        match id {
            VideoId::Bvid(bvid) => {
                endpoint.query_pairs_mut().append_pair("bvid", bvid);
            }
            VideoId::Aid(aid) => {
                endpoint
                    .query_pairs_mut()
                    .append_pair("aid", &aid.to_string());
            }
        }
        let value = self.request_api(endpoint).await?;
        let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
        if code != 0 {
            return match code {
                -404 | 62002 => Err(Error::NotFound),
                -412 | -101 => Err(Error::LoginRequired),
                _ => Err(Error::UpstreamChanged),
            };
        }
        value.get("data").cloned().ok_or(Error::UpstreamChanged)
    }

    /// Requests progressive sources first, then DASH sources as a fallback.
    async fn request_play_sources(&self, bvid: &str, cid: u64) -> Result<Vec<MediaSource>> {
        // `fnval=1` requests progressive media; 16, 80, and 4048 enable DASH variants.
        const ATTEMPTS: &[(u32, u32)] =
            &[(1, 80), (1, 64), (1, 32), (16, 80), (80, 80), (4048, 80)];
        let mut last_error = None;
        let mut collected = Vec::new();
        for &(fnval, qn) in ATTEMPTS {
            match self.request_playurl_raw(bvid, cid, fnval, qn).await {
                Ok(play) => {
                    for source in collect_play_sources(&play) {
                        if !collected
                            .iter()
                            .any(|existing: &MediaSource| existing.url == source.url)
                        {
                            collected.push(source);
                        }
                    }
                    if fnval == 1 && !collected.is_empty() {
                        return Ok(collected);
                    }
                }
                Err(error) => last_error = Some(error),
            }
        }
        if collected.is_empty() {
            Err(last_error.unwrap_or(Error::MediaUnavailable))
        } else {
            Ok(collected)
        }
    }

    async fn request_playurl_raw(
        &self,
        bvid: &str,
        cid: u64,
        fnval: u32,
        qn: u32,
    ) -> Result<Value> {
        let mut endpoint = Url::parse(PLAYURL_API)
            .map_err(|_| Error::Config("哔哩哔哩 playurl API 地址无效".into()))?;
        endpoint
            .query_pairs_mut()
            .append_pair("bvid", bvid)
            .append_pair("cid", &cid.to_string())
            .append_pair("qn", &qn.to_string())
            .append_pair("fnval", &fnval.to_string())
            .append_pair("fourk", "1")
            .append_pair("fnver", "0");
        let value = self.request_api(endpoint).await?;
        if value.get("code").and_then(Value::as_i64).unwrap_or(-1) != 0 {
            return Err(Error::MediaUnavailable);
        }
        value.get("data").cloned().ok_or(Error::MediaUnavailable)
    }

    async fn request_api(&self, endpoint: Url) -> Result<Value> {
        let response = self
            .client
            .get(endpoint)
            .header(ACCEPT, "application/json")
            .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .header(REFERER, "https://www.bilibili.com/")
            .send()
            .await
            .map_err(|error| map_network_error(&error, "上游请求超时", "无法连接上游服务"))?;
        map_status(response.status())?;
        let bytes = read_body_limited(response, MAX_JSON_BYTES, |error| {
            map_network_error(error, "上游请求超时", "无法连接上游服务")
        })
        .await?;
        serde_json::from_slice(&bytes).map_err(|_| Error::UpstreamChanged)
    }
}

impl PlatformResolver for BilibiliResolver {
    fn platform_id(&self) -> PlatformId {
        PlatformId::Bilibili
    }

    fn extract_share_url(&self, input: &str) -> Result<Url> {
        extract_share_url(input)
    }

    async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        BilibiliResolver::resolve_text(self, input).await
    }

    async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        BilibiliResolver::resolve_url(self, url).await
    }
}

fn map_status(status: StatusCode) -> Result<()> {
    match status {
        status if status.is_success() => Ok(()),
        StatusCode::TOO_MANY_REQUESTS => Err(Error::RateLimited),
        StatusCode::NOT_FOUND | StatusCode::GONE => Err(Error::NotFound),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Error::LoginRequired),
        _ => Err(Error::Network(format!("上游返回 HTTP {}", status.as_u16()))),
    }
}
