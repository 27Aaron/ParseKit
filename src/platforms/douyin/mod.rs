//! Resolve Douyin videos from short links and embedded page data.

mod parse;
mod share;

#[cfg(test)]
mod tests;

use std::time::Duration;

use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, ACCEPT_LANGUAGE, LOCATION, USER_AGENT},
    redirect::Policy,
};
use url::Url;

use crate::{
    Error, PlatformId, ResolvedPost, Result,
    media::DownloadRequestIdentity,
    platforms::{
        PlatformResolver,
        util::{map_network_error, read_body_limited},
    },
};

use self::{
    parse::{build_post_from_router, parse_any_page_data, parse_router_data},
    share::{extract_aweme_id, is_allowed_redirect_host, is_short_link_host},
};

pub use self::share::extract_share_url;

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SHORTLINK_REDIRECTS: usize = 8;
const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MOBILE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) \
    AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1";
const MEDIA_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const DOUYIN_ORIGIN: &str = "https://www.douyin.com";
const DOUYIN_REFERER: &str = "https://www.douyin.com/";

/// Reviewed media hosts; a leading dot matches subdomains only.
pub const REVIEWED_MEDIA_HOSTS: &[&str] = &[
    "aweme.snssdk.com",
    "www.douyin.com",
    "www.iesdouyin.com",
    ".douyinvod.com",
    ".douyincdn.com",
    // Play endpoints sometimes redirect through jspcdn edges (often :20443).
    ".jspcdn.cn",
    ".bytevcloudcdn.com",
    ".bytecdn.cn",
    ".bytecdn.com",
    ".zjcdn.com",
    ".douyinpic.com",
    ".ibyteimg.com",
    ".pstatp.com",
];

pub const REVIEWED_DOUYIN_MEDIA_HOSTS: &[&str] = REVIEWED_MEDIA_HOSTS;

const REDIRECT_HOSTS: &[&str] = &[
    "v.douyin.com",
    "www.douyin.com",
    "m.douyin.com",
    "www.iesdouyin.com",
    "iesdouyin.com",
];

pub fn download_identity() -> DownloadRequestIdentity {
    DownloadRequestIdentity {
        origin: Some(DOUYIN_ORIGIN.to_owned()),
        referer: Some(DOUYIN_REFERER.to_owned()),
        user_agent: Some(MEDIA_USER_AGENT.to_owned()),
    }
}

fn map_douyin_network_error(error: &reqwest::Error) -> Error {
    map_network_error(error, "抖音请求超时", "抖音网络请求失败")
}

#[derive(Clone)]
pub struct DouyinResolver {
    client: Client,
    timeout: Duration,
}

impl std::fmt::Debug for DouyinResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DouyinResolver")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl DouyinResolver {
    pub fn new() -> Result<Self> {
        let timeout = RESOLVE_TIMEOUT;
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .redirect(Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| Error::Config("无法初始化抖音 HTTP 客户端".into()))?;
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
        tokio::time::timeout(self.timeout, self.resolve_url_inner(url))
            .await
            .map_err(|_| Error::Network("抖音解析总超时".into()))?
    }

    async fn resolve_url_inner(&self, url: &Url) -> Result<ResolvedPost> {
        let expanded = self.expand_short_link(url).await?;
        let aweme_id = extract_aweme_id(expanded.as_str()).ok_or(Error::UnsupportedUrl)?;
        let html = self.fetch_share_html(&aweme_id).await?;
        match parse_router_data(&html).and_then(|router| build_post_from_router(&aweme_id, &router))
        {
            Ok(post) => Ok(post),
            Err(primary_error) => {
                tracing::debug!(
                    event = "douyin_primary_parse_failed",
                    error = %primary_error,
                    "iesdouyin share parse failed; trying www.douyin.com fallback"
                );
                let fallback_html = match self.fetch_www_video_html(&aweme_id).await {
                    Ok(html) => html,
                    Err(_) => return Err(primary_error),
                };
                let router = parse_any_page_data(&fallback_html).map_err(|_| primary_error)?;
                build_post_from_router(&aweme_id, &router)
            }
        }
    }

    async fn expand_short_link(&self, url: &Url) -> Result<Url> {
        if !is_short_link_host(url) {
            return Ok(url.clone());
        }

        let mut current = url.clone();
        for _ in 0..MAX_SHORTLINK_REDIRECTS {
            if !is_allowed_redirect_host(&current) {
                return Err(Error::Network("抖音短链跳转到了未允许的主机".into()));
            }

            let response = self
                .client
                .get(current.clone())
                .header(USER_AGENT, MOBILE_UA)
                .header(ACCEPT, "text/html,application/xhtml+xml")
                .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
                .send()
                .await
                .map_err(|error| map_douyin_network_error(&error))?;

            if !response.status().is_redirection() {
                return Ok(response.url().clone());
            }

            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| Error::Network("抖音短链缺少 Location".into()))?;
            current = current
                .join(location)
                .map_err(|_| Error::Network("抖音短链 Location 无效".into()))?;

            if extract_aweme_id(current.as_str()).is_some() {
                return Ok(current);
            }
        }

        Err(Error::Network("抖音短链重定向次数过多".into()))
    }

    async fn fetch_share_html(&self, aweme_id: &str) -> Result<String> {
        let share_url = Url::parse(&format!("https://www.iesdouyin.com/share/video/{aweme_id}"))
            .map_err(|_| Error::UpstreamChanged)?;
        self.fetch_html(share_url).await
    }

    async fn fetch_www_video_html(&self, aweme_id: &str) -> Result<String> {
        let page = Url::parse(&format!("https://www.douyin.com/video/{aweme_id}"))
            .map_err(|_| Error::UpstreamChanged)?;
        self.fetch_html(page).await
    }

    async fn fetch_html(&self, url: Url) -> Result<String> {
        let response = self
            .client
            .get(url)
            .header(USER_AGENT, MOBILE_UA)
            .header(ACCEPT, "text/html,application/xhtml+xml")
            .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
            .send()
            .await
            .map_err(|error| map_douyin_network_error(&error))?;

        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(Error::RateLimited);
        }
        if status == StatusCode::NOT_FOUND {
            return Err(Error::NotFound);
        }
        if !status.is_success() {
            return Err(Error::Network(format!("抖音页面 HTTP {}", status.as_u16())));
        }

        let bytes = read_body_limited(response, MAX_HTML_BYTES, map_douyin_network_error).await?;
        String::from_utf8(bytes).map_err(|_| Error::UpstreamChanged)
    }
}

impl PlatformResolver for DouyinResolver {
    fn platform_id(&self) -> PlatformId {
        PlatformId::Douyin
    }

    fn extract_share_url(&self, input: &str) -> Result<Url> {
        extract_share_url(input)
    }

    async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        DouyinResolver::resolve_text(self, input).await
    }

    async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        DouyinResolver::resolve_url(self, url).await
    }
}
