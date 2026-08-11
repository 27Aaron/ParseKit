//! WeChat Channels resolve (Yuanbao parse + feed).

mod build;
mod share;
mod util;

#[cfg(test)]
mod tests;

use std::{sync::Arc, time::Duration};

use chrono::Utc;
use reqwest::{
    Client,
    header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, COOKIE, ORIGIN, REFERER, USER_AGENT},
    redirect::Policy,
};
use serde_json::Value;
use url::Url;
use uuid::Uuid;

use crate::{Error, ResolvedPost, Result, platforms::PlatformResolver};

use self::build::build_post;
use self::share::{endpoint_is_loopback_http, normalize_share_url, query_value};
use self::util::{
    FeedBaseRequest, FeedRequest, ParseData, ParseRequest, cookie_value, map_network_error,
    map_status, non_empty, read_json, response_looks_like_login, value_to_text,
};

pub use self::share::{derive_direct_media_url, extract_share_url};

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
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct WechatResolver {
    client: Client,
    cookie: Arc<str>,
    parse_endpoint: Url,
    feed_endpoint: Url,
    timeout: Duration,
}

impl std::fmt::Debug for WechatResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WechatResolver")
            .field("cookie", &"<redacted>")
            .field("endpoints", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl WechatResolver {
    pub fn new(cookie: impl Into<String>) -> Result<Self> {
        Self::with_endpoints(
            cookie,
            Url::parse(PARSE_ENDPOINT).expect("constant parse endpoint must be valid"),
            Url::parse(FEED_ENDPOINT).expect("constant feed endpoint must be valid"),
        )
    }

    fn with_endpoints(
        cookie: impl Into<String>,
        parse_endpoint: Url,
        feed_endpoint: Url,
    ) -> Result<Self> {
        let timeout = RESOLVE_TIMEOUT;
        let cookie = cookie.into();
        if cookie.trim().is_empty() {
            return Err(Error::Config("YUANBAO_COOKIE 不能为空".into()));
        }
        for endpoint in [&parse_endpoint, &feed_endpoint] {
            if endpoint.scheme() != "https" && !endpoint_is_loopback_http(endpoint) {
                return Err(Error::Config("视频号解析 endpoint 必须使用 HTTPS".into()));
            }
        }

        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout)
            .redirect(Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| Error::Config("无法初始化视频号 HTTP 客户端".into()))?;

        Ok(Self {
            client,
            cookie: Arc::from(cookie),
            parse_endpoint,
            feed_endpoint,
            timeout,
        })
    }

    pub async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        let url = extract_share_url(input)?;
        let normalized = normalize_share_url(&url)?;
        tokio::time::timeout(self.timeout, self.resolve_normalized(normalized))
            .await
            .map_err(|_| Error::Network("视频号解析总超时".into()))?
    }

    pub async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        let normalized = normalize_share_url(url)?;
        tokio::time::timeout(self.timeout, self.resolve_normalized(normalized))
            .await
            .map_err(|_| Error::Network("视频号解析总超时".into()))?
    }

    async fn resolve_normalized(
        &self,
        normalized: self::share::NormalizedShareUrl,
    ) -> Result<ResolvedPost> {
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
            .header(COOKIE, self.cookie.as_ref())
            .json(&payload);

        if let Some(user_id) = cookie_value(&self.cookie, "hy_user") {
            request = request.header("t-userid", &user_id).header("x-id", user_id);
        }
        if let Some(device_id) = cookie_value(&self.cookie, "_qimei_uuid42") {
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

        let code = value.get("code").and_then(Value::as_i64).unwrap_or(0);
        if code != 0 {
            return if response_looks_like_login(&value) {
                Err(Error::LoginRequired)
            } else {
                Err(Error::UpstreamChanged)
            };
        }

        let data = value.get("data").cloned().ok_or_else(|| {
            if response_looks_like_login(&value) {
                Error::LoginRequired
            } else {
                Error::UpstreamChanged
            }
        })?;
        serde_json::from_value::<ParseData>(data).map_err(|_| Error::UpstreamChanged)
    }

    async fn request_feed(&self, export_id: &str, general_token: &str) -> Result<Value> {
        let rid = format!(
            "{:x}-{}",
            Utc::now().timestamp(),
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

        let err_code = value
            .get("errCode")
            .or_else(|| value.get("errcode"))
            .and_then(Value::as_i64)
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

impl PlatformResolver for WechatResolver {
    fn platform_id(&self) -> &'static str {
        "wechat_channels"
    }

    fn extract_share_url(&self, input: &str) -> Result<Url> {
        extract_share_url(input)
    }

    async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        WechatResolver::resolve_text(self, input).await
    }

    async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        WechatResolver::resolve_url(self, url).await
    }
}
