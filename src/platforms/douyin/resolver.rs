//! Network orchestration for Douyin videos.

use std::time::Duration;

use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, ACCEPT_LANGUAGE, LOCATION, USER_AGENT},
};
use url::Url;

use crate::{
    Error, ResolvedPost, Result,
    platforms::{
        PlatformResolver, PlatformSpec,
        util::{
            DEFAULT_RESOLVE_TIMEOUT, map_network_error, read_body_limited, resolve_with_timeout,
            resolver_http_client,
        },
    },
};

use super::{
    SPEC, extract_share_url,
    parse::{build_post_from_router, parse_any_page_data, parse_router_data},
    share::{extract_aweme_id, is_allowed_redirect_host, is_short_link_host},
};

const RESOLVE_TIMEOUT: Duration = DEFAULT_RESOLVE_TIMEOUT;
const MAX_SHORTLINK_REDIRECTS: usize = 8;
const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MOBILE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) \
    AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1";
const LIVE_RESOLUTION_ENABLED: bool = false;
const UNAVAILABLE_MESSAGE: &str = "抖音现要求动态浏览器验证；当前版本已暂停解析，避免返回错误媒体";

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
        let client = resolver_http_client(timeout, "无法初始化抖音 HTTP 客户端")?;
        Ok(Self { client, timeout })
    }

    pub async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        let url = extract_share_url(input)?;
        self.resolve_url(&url).await
    }

    pub async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        let url = extract_share_url(url.as_str())?;
        self.resolve_share_url(&url).await
    }

    async fn resolve_share_url(&self, url: &Url) -> Result<ResolvedPost> {
        resolve_with_timeout(self.timeout, self.resolve_url_inner(url), "抖音解析总超时").await
    }

    async fn resolve_url_inner(&self, url: &Url) -> Result<ResolvedPost> {
        if !LIVE_RESOLUTION_ENABLED {
            return Err(Error::PlatformUnavailable(UNAVAILABLE_MESSAGE.to_owned()));
        }

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

            let status = response.status();
            if !status.is_redirection() {
                return match status {
                    StatusCode::TOO_MANY_REQUESTS => Err(Error::RateLimited),
                    StatusCode::NOT_FOUND | StatusCode::GONE => Err(Error::NotFound),
                    status if !status.is_success() => Err(Error::Network(format!(
                        "抖音短链返回 HTTP {}",
                        status.as_u16()
                    ))),
                    _ => extract_aweme_id(response.url().as_str())
                        .map(|_| response.url().clone())
                        .ok_or(Error::UpstreamChanged),
                };
            }

            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| Error::Network("抖音短链缺少 Location".into()))?;
            current = current
                .join(location)
                .map_err(|_| Error::Network("抖音短链 Location 无效".into()))?;

            if !is_allowed_redirect_host(&current) {
                return Err(Error::Network("抖音短链跳转到了未允许的主机".into()));
            }

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
    fn spec(&self) -> &'static PlatformSpec {
        &SPEC
    }

    async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        DouyinResolver::resolve_url(self, url).await
    }
}
