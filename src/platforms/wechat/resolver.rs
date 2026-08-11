//! Network orchestration for WeChat Channels links.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::{
    Client,
    header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, COOKIE, ORIGIN, REFERER, USER_AGENT},
};
use serde::Deserialize as _;
use serde_json::Value;
use url::Url;
use uuid::Uuid;

use crate::{
    Error, ResolvedPost, Result,
    auth::{CookieCredential, CredentialStatus},
    platforms::{
        PlatformResolver, PlatformSpec,
        util::{DEFAULT_RESOLVE_TIMEOUT, resolve_with_timeout, resolver_http_client},
    },
};

use super::{
    SPEC,
    api::{
        FeedBaseRequest, FeedRequest, ParseData, ParseRequest, cookie_value, integer_at,
        map_network_error, map_status, non_empty, read_json, response_looks_like_login,
        value_to_text,
    },
    parse::build_post,
    share::{
        NormalizedShareUrl, endpoint_is_loopback_http, extract_share_url, normalize_share_url,
        query_value,
    },
};

const PARSE_ENDPOINT: &str = "https://yuanbao.tencent.com/api/weixin/get_parse_result";
const FEED_ENDPOINT: &str = "https://channels.weixin.qq.com/finder-preview/api/feed/get_feed_info";
const YUANBAO_ORIGIN: &str = "https://yuanbao.tencent.com";
const CHANNELS_ORIGIN: &str = "https://channels.weixin.qq.com";
const YUANBAO_AGENT_ID: &str = "naQivTmsDa/cf4d0079-ed1b-4c55-a3f3-2ca1379727d1";
const YUANBAO_REFERER: &str =
    "https://yuanbao.tencent.com/chat/naQivTmsDa/cf4d0079-ed1b-4c55-a3f3-2ca1379727d1";
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";
const SEC_CH_UA_VALUE: &str =
    r#""Chromium";v="148", "Google Chrome";v="148", "Not/A)Brand";v="99""#;
const RESOLVE_TIMEOUT: Duration = DEFAULT_RESOLVE_TIMEOUT;

#[derive(Clone)]
pub struct WechatResolver {
    client: Client,
    cookie: CookieCredential,
    parse_endpoint: Url,
    feed_endpoint: Url,
    timeout: Duration,
}

impl std::fmt::Debug for WechatResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WechatResolver")
            .field("cookie", &self.cookie)
            .field("endpoints", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Local assessment of Yuanbao cookie markers.
///
/// Prefer [`CredentialStatus`] for new code; this alias keeps older call sites working.
pub type WechatCredentialStatus = CredentialStatus;

impl WechatResolver {
    pub fn new(cookie: impl Into<String>) -> Result<Self> {
        Self::with_endpoints(
            cookie,
            Url::parse(PARSE_ENDPOINT).expect("constant parse endpoint must be valid"),
            Url::parse(FEED_ENDPOINT).expect("constant feed endpoint must be valid"),
        )
    }

    /// Local cookie shape check (no network).
    ///
    /// Returns [`CredentialStatus::Present`] when `hy_user` / session tokens look set;
    /// [`CredentialStatus::Incomplete`] otherwise. Never returns `Absent` (resolver always
    /// holds a cookie string).
    pub fn credential_status(&self) -> CredentialStatus {
        assess_yuanbao_cookie(self.cookie.as_str())
    }

    fn with_endpoints(
        cookie: impl Into<String>,
        parse_endpoint: Url,
        feed_endpoint: Url,
    ) -> Result<Self> {
        let timeout = RESOLVE_TIMEOUT;
        let cookie = CookieCredential::new(cookie)
            .ok_or_else(|| Error::Config("YUANBAO_COOKIE 不能为空".into()))?;
        for endpoint in [&parse_endpoint, &feed_endpoint] {
            if endpoint.scheme() != "https" && !endpoint_is_loopback_http(endpoint) {
                return Err(Error::Config("视频号解析 endpoint 必须使用 HTTPS".into()));
            }
        }

        let client = resolver_http_client(timeout, "无法初始化视频号 HTTP 客户端")?;

        Ok(Self {
            client,
            cookie,
            parse_endpoint,
            feed_endpoint,
            timeout,
        })
    }

    /// Cookie header value for outbound requests (redacted in Debug).
    pub fn cookie_header(&self) -> &str {
        self.cookie.as_str()
    }

    pub async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        let url = extract_share_url(input)?;
        self.resolve_url(&url).await
    }

    pub async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        let normalized = normalize_share_url(url)?;
        self.resolve_share_url(normalized).await
    }

    async fn resolve_share_url(&self, normalized: NormalizedShareUrl) -> Result<ResolvedPost> {
        resolve_with_timeout(
            self.timeout,
            self.resolve_normalized(normalized),
            "视频号解析总超时",
        )
        .await
    }

    async fn resolve_normalized(&self, normalized: NormalizedShareUrl) -> Result<ResolvedPost> {
        let parse_data = self.request_parse(&normalized.canonical_url).await?;
        let playable_url =
            Url::parse(parse_data.playable_url.trim()).map_err(|_| Error::UpstreamChanged)?;
        let general_token = query_value(&playable_url, "token").ok_or(Error::UpstreamChanged)?;
        let export_id = query_value(&playable_url, "eid")
            .or_else(|| non_empty(parse_data.wx_export_id.clone()))
            .ok_or(Error::UpstreamChanged)?;

        let feed = self.request_feed(&export_id, &general_token).await?;
        build_post(normalized, parse_data, feed, export_id)
    }

    async fn request_parse(&self, share_url: &Url) -> Result<ParseData> {
        let payload = ParseRequest {
            kind: "video_channel_url",
            url: share_url.as_str(),
            scene: 1,
        };

        let mut request = self
            .client
            .post(self.parse_endpoint.clone())
            .header(ACCEPT, "application/json, text/plain, */*")
            .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
            .header(CONTENT_TYPE, "application/json")
            .header(ORIGIN, YUANBAO_ORIGIN)
            .header(REFERER, YUANBAO_REFERER)
            .header(USER_AGENT, USER_AGENT_VALUE)
            .header("sec-ch-ua", SEC_CH_UA_VALUE)
            .header("sec-ch-ua-mobile", "?0")
            .header("sec-ch-ua-platform", r#""macOS""#)
            .header("sec-fetch-dest", "empty")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-site", "same-origin")
            .header("x-agentid", YUANBAO_AGENT_ID)
            .header("x-instance-id", "5")
            .header("x-language", "zh-CN")
            .header("x-os_version", "Mac OS(10.15.7)-Blink")
            .header("x-platform", "mac")
            .header("x-requested-with", "XMLHttpRequest")
            .header("x-source", "web")
            .header("x-web-third-source", "main")
            .header("x-webdriver", "0")
            .header("x-webversion", "2.69.0")
            .header("x-ybuitest", "0")
            .header(COOKIE, self.cookie.as_str())
            .json(&payload);

        if let Some(user_id) = cookie_value(self.cookie.as_str(), "hy_user") {
            request = request.header("t-userid", &user_id).header("x-id", user_id);
        }
        if let Some(device_id) = cookie_value(self.cookie.as_str(), "_qimei_uuid42") {
            request = request
                .header("x-device-id", &device_id)
                .header("x-hy93", device_id);
        }

        let response = request
            .send()
            .await
            .map_err(|error| map_network_error(&error))?;
        map_status(response.status(), true)?;
        let value = read_json(response).await?;

        let code = integer_at(&value, "code").unwrap_or(0);
        if code != 0 {
            return if response_looks_like_login(&value) {
                Err(Error::LoginRequired)
            } else {
                Err(Error::UpstreamChanged)
            };
        }

        let data = value.get("data").ok_or_else(|| {
            if response_looks_like_login(&value) {
                Error::LoginRequired
            } else {
                Error::UpstreamChanged
            }
        })?;
        ParseData::deserialize(data).map_err(|_| Error::UpstreamChanged)
    }

    async fn request_feed(&self, export_id: &str, general_token: &str) -> Result<Value> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Config("系统时间早于 Unix epoch".into()))?
            .as_secs();
        let rid = format!(
            "{:x}-{}",
            timestamp,
            &Uuid::new_v4().simple().to_string()[..8]
        );
        let mut endpoint = self.feed_endpoint.clone();
        endpoint
            .query_pairs_mut()
            .append_pair("_rid", &rid)
            .append_pair(
                "_pageUrl",
                "https://channels.weixin.qq.com/finder-preview/pages/feed",
            );

        let mut referer = Url::parse("https://channels.weixin.qq.com/finder-preview/pages/feed")
            .expect("constant referer must be valid");
        referer
            .query_pairs_mut()
            .append_pair("entry_card_type", "48")
            .append_pair("comment_scene", "39")
            .append_pair("appid", "0")
            .append_pair("token", general_token)
            .append_pair("entry_scene", "0")
            .append_pair("eid", export_id);

        let response = self
            .client
            .post(endpoint)
            .header(ACCEPT, "application/json, text/plain, */*")
            .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
            .header(CONTENT_TYPE, "application/json")
            .header(ORIGIN, CHANNELS_ORIGIN)
            .header(REFERER, referer.as_str())
            .header(USER_AGENT, USER_AGENT_VALUE)
            .header("sec-ch-ua", SEC_CH_UA_VALUE)
            .header("sec-ch-ua-mobile", "?0")
            .header("sec-ch-ua-platform", r#""macOS""#)
            .header("sec-fetch-dest", "empty")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-site", "same-origin")
            .json(&FeedRequest {
                base_req: FeedBaseRequest { general_token },
                export_id,
            })
            .send()
            .await
            .map_err(|error| map_network_error(&error))?;
        map_status(response.status(), false)?;
        let value = read_json(response).await?;

        let err_code = integer_at(&value, "errCode")
            .or_else(|| integer_at(&value, "errcode"))
            .unwrap_or(0);
        if err_code != 0 {
            return if response_looks_like_login(&value) {
                Err(Error::LoginRequired)
            } else if value_to_text(value.get("errMsg")).contains("不存在") {
                Err(Error::NotFound)
            } else {
                Err(Error::UpstreamChanged)
            };
        }
        Ok(value)
    }
}

/// Local shape check for a Yuanbao cookie header (no network).
pub fn assess_yuanbao_cookie(cookie: &str) -> CredentialStatus {
    let trimmed = cookie.trim();
    if trimmed.is_empty() {
        return CredentialStatus::Absent;
    }
    let has_user = cookie_value(trimmed, "hy_user").is_some();
    let has_session = cookie_value(trimmed, "token").is_some()
        || cookie_value(trimmed, "hy_token").is_some()
        || cookie_value(trimmed, "yuanbao_token").is_some();
    if has_user && has_session {
        CredentialStatus::Present
    } else {
        CredentialStatus::Incomplete
    }
}

impl PlatformResolver for WechatResolver {
    fn spec(&self) -> &'static PlatformSpec {
        &SPEC
    }

    async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        WechatResolver::resolve_url(self, url).await
    }
}
