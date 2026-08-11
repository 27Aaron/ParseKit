//! Network orchestration for public Bilibili videos.

use std::{collections::HashSet, time::Duration};

use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, ACCEPT_LANGUAGE, COOKIE, LOCATION, REFERER, USER_AGENT},
};
use serde_json::Value;
use url::Url;

use crate::{
    Error, Result,
    auth::{CookieCredential, CredentialStatus, cookie_value},
    model::{MediaSource, ResolvedPost},
    platforms::{
        PlatformResolver, PlatformSpec,
        util::{
            DEFAULT_RESOLVE_TIMEOUT, map_network_error, read_body_limited, resolve_with_timeout,
            resolver_http_client,
        },
    },
    url::{CleanPolicy, clean_tracking_params},
};

use super::{
    SPEC, extract_share_url,
    hosts::USER_AGENT_VALUE,
    parse::{build_post_from_view, collect_play_sources},
    share::{VideoId, is_b23_host, parse_video_id},
};

const RESOLVE_TIMEOUT: Duration = DEFAULT_RESOLVE_TIMEOUT;
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_SHORTLINK_REDIRECTS: usize = 8;
const VIEW_API: &str = "https://api.bilibili.com/x/web-interface/view";
const PLAYURL_API: &str = "https://api.bilibili.com/x/player/playurl";

/// Anonymous ladder: progressive first, then DASH.
const ANON_PLAY_ATTEMPTS: &[(u32, u32)] =
    &[(1, 80), (1, 64), (1, 32), (16, 80), (80, 80), (4048, 80)];
/// Authenticated ladder: DASH first, then muxed progressive.
const AUTH_PLAY_ATTEMPTS: &[(u32, u32)] = &[(4048, 127), (80, 112), (1, 80)];

#[derive(Clone)]
pub struct BilibiliResolver {
    client: Client,
    timeout: Duration,
    cookie: Option<CookieCredential>,
}

impl std::fmt::Debug for BilibiliResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BilibiliResolver")
            .field("timeout", &self.timeout)
            .field("cookie", &self.cookie.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

impl BilibiliResolver {
    /// Creates an anonymous resolver with public quality limits.
    pub fn new() -> Result<Self> {
        Self::with_cookie_opt(None)
    }

    /// Creates a resolver with an optional Cookie header.
    pub fn with_cookie(cookie: impl Into<String>) -> Result<Self> {
        let cookie = cookie.into();
        let cred = CookieCredential::new(cookie)
            .ok_or_else(|| Error::Config("BILIBILI_COOKIE 不能为空".into()))?;
        Self::with_cookie_opt(Some(cred))
    }

    fn with_cookie_opt(cookie: Option<CookieCredential>) -> Result<Self> {
        let timeout = RESOLVE_TIMEOUT;
        let client = resolver_http_client(timeout, "无法初始化哔哩哔哩 HTTP 客户端")?;
        Ok(Self {
            client,
            timeout,
            cookie,
        })
    }

    /// Returns the locally inferred credential state.
    pub fn credential_status(&self) -> CredentialStatus {
        match &self.cookie {
            None => CredentialStatus::Absent,
            Some(cookie) => {
                let has_sess = cookie_value(cookie.as_str(), "SESSDATA")
                    .is_some_and(|value| !value.is_empty());
                if has_sess {
                    CredentialStatus::Present
                } else {
                    CredentialStatus::Incomplete
                }
            }
        }
    }

    /// Returns whether API requests include a session cookie.
    pub fn is_authenticated(&self) -> bool {
        matches!(self.credential_status(), CredentialStatus::Present)
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
        resolve_with_timeout(
            self.timeout,
            self.resolve_normalized(url),
            "哔哩哔哩解析总超时",
        )
        .await
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
        let mut value = self.request_api(endpoint).await?;
        let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
        if code != 0 {
            return match code {
                -404 | 62002 => Err(Error::NotFound),
                -101 => Err(Error::LoginRequired),
                -412 => Err(Error::RateLimited),
                _ => Err(Error::UpstreamChanged),
            };
        }
        value
            .get_mut("data")
            .map(Value::take)
            .ok_or(Error::UpstreamChanged)
    }

    /// Merges muxed progressive and authenticated DASH ladders.
    pub(super) async fn request_play_sources(
        &self,
        bvid: &str,
        cid: u64,
    ) -> Result<Vec<MediaSource>> {
        let authenticated = self.is_authenticated();
        let attempts = if authenticated {
            AUTH_PLAY_ATTEMPTS
        } else {
            ANON_PLAY_ATTEMPTS
        };
        let mut last_error = None;
        let mut collected = Vec::new();
        let mut seen_urls = HashSet::new();
        let mut got_dash = false;
        for &(fnval, qn) in attempts {
            // After DASH succeeds, only the muxed progressive option remains useful.
            if authenticated && got_dash && fnval != 1 {
                continue;
            }
            match self.request_playurl_raw(bvid, cid, fnval, qn).await {
                Ok(play) => {
                    let before = collected.len();
                    for source in collect_play_sources(&play) {
                        if seen_urls.insert(source.url.clone()) {
                            collected.push(source);
                        }
                    }
                    let added = collected.len() > before;
                    if fnval != 1 && added {
                        got_dash = true;
                    }
                    // Anonymous resolution stops after the first progressive source.
                    if !authenticated && fnval == 1 && !collected.is_empty() {
                        return Ok(collected);
                    }
                }
                Err(error) => last_error = Some(error),
            }
        }
        if collected.is_empty() {
            Err(last_error.unwrap_or(Error::MediaUnavailable))
        } else {
            Ok(dedupe_and_rank_play_sources(collected))
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
        let mut value = self.request_api(endpoint).await?;
        match value.get("code").and_then(Value::as_i64).unwrap_or(-1) {
            0 => {}
            -101 => return Err(Error::LoginRequired),
            -404 | 62002 => return Err(Error::NotFound),
            -412 => return Err(Error::RateLimited),
            _ => return Err(Error::MediaUnavailable),
        }
        value
            .get_mut("data")
            .map(Value::take)
            .ok_or(Error::MediaUnavailable)
    }

    async fn request_api(&self, endpoint: Url) -> Result<Value> {
        let mut request = self
            .client
            .get(endpoint)
            .header(ACCEPT, "application/json")
            .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .header(REFERER, "https://www.bilibili.com/");
        if let Some(cookie) = &self.cookie {
            request = request.header(COOKIE, cookie.as_str());
        }
        let response = request
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
    fn spec(&self) -> &'static PlatformSpec {
        &SPEC
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

/// Keeps the highest-bitrate stream for each quality key.
fn dedupe_and_rank_play_sources(sources: Vec<MediaSource>) -> Vec<MediaSource> {
    let mut ranked = sources;
    ranked.sort_by(|a, b| {
        let area = |s: &MediaSource| {
            s.width
                .and_then(|w| s.height.map(|h| u64::from(w) * u64::from(h)))
                .unwrap_or(0)
        };
        area(b)
            .cmp(&area(a))
            .then_with(|| b.bitrate_bps.unwrap_or(0).cmp(&a.bitrate_bps.unwrap_or(0)))
            .then_with(|| b.size_hint.unwrap_or(0).cmp(&a.size_hint.unwrap_or(0)))
    });

    let mut seen = HashSet::with_capacity(ranked.len());
    let mut unique = Vec::with_capacity(ranked.len());
    for source in ranked {
        let key = (
            source.label.clone().unwrap_or_default(),
            source.width.unwrap_or(0),
            source.height.unwrap_or(0),
        );
        if seen.insert(key) {
            unique.push(source);
        }
    }
    unique
}
