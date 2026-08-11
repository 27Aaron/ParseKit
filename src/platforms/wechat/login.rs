//! Yuanbao web QR login through WeChat Open Platform.
//!
//! The flow mirrors Yuanbao's current web client:
//! 1. create a `qrconnect` page and extract its short-lived QR UUID;
//! 2. display the JPEG served by WeChat;
//! 3. long-poll WeChat until the user confirms;
//! 4. exchange the returned authorization code with Yuanbao and retain its cookies.

use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU16, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use regex::Regex;
use reqwest::{
    Client,
    cookie::{CookieStore, Jar},
    header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, ORIGIN, REFERER, USER_AGENT},
    redirect::Policy,
};
use serde::Serialize;
use serde_json::Value;
use url::Url;
use uuid::Uuid;

use crate::{
    Error, Result,
    auth::{CookieCredential, CredentialStatus},
    platforms::util::{map_network_error, read_body_limited},
};

use super::resolver::assess_yuanbao_cookie;

const WECHAT_APP_ID: &str = "wx12b75947931a04ec";
const WECHAT_QRCONNECT_URL: &str = "https://open.weixin.qq.com/connect/qrconnect";
const WECHAT_QR_IMAGE_ORIGIN: &str = "https://open.weixin.qq.com";
const WECHAT_QR_POLL_URL: &str = "https://lp.open.weixin.qq.com/connect/l/qrconnect";
const YUANBAO_LOGIN_URL: &str = "https://yuanbao.tencent.com/api/joint/login";
const YUANBAO_SCAN_URL: &str = "https://yuanbao.tencent.com/scan";
const YUANBAO_ORIGIN: &str = "https://yuanbao.tencent.com";
const YUANBAO_REFERER: &str = "https://yuanbao.tencent.com/chat/naQivTmsDa";
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";
const SEC_CH_UA_VALUE: &str =
    r#""Chromium";v="148", "Google Chrome";v="148", "Not/A)Brand";v="99""#;
const MAX_QR_PAGE_BYTES: usize = 512 * 1024;
const MAX_QR_IMAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_POLL_BYTES: usize = 16 * 1024;
const MAX_LOGIN_BYTES: usize = 256 * 1024;
const NO_LAST_STATUS: u16 = 0;

/// A short-lived Yuanbao QR login session.
///
/// The QR image and its URL are intentionally absent from [`Debug`] because the
/// embedded UUID is an active login credential until it expires.
pub struct QrLoginSession {
    client: Client,
    cookies: Arc<Jar>,
    connect_url: Url,
    callback_url: Url,
    qrcode_url: Url,
    qrcode_image: Vec<u8>,
    uuid: String,
    last_status: AtomicU16,
}

impl QrLoginSession {
    /// JPEG bytes for the QR code shown to the user.
    pub fn qrcode_image(&self) -> &[u8] {
        &self.qrcode_image
    }

    /// Short-lived image URL, useful as a fallback when a terminal cannot render the JPEG.
    pub fn qrcode_url(&self) -> &Url {
        &self.qrcode_url
    }
}

impl std::fmt::Debug for QrLoginSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QrLoginSession")
            .field("qrcode", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// One poll of the WeChat QR authorization state.
#[derive(Debug, Clone)]
pub enum QrPollStatus {
    /// Waiting for the QR code to be scanned.
    WaitingScan,
    /// Scanned; waiting for confirmation in WeChat.
    WaitingConfirm,
    /// The user rejected the confirmation. The same QR can still be scanned again.
    Cancelled,
    /// The QR code expired and a new session is required.
    Expired,
    /// Yuanbao accepted the WeChat code and returned a usable cookie.
    Success(CookieCredential),
}

/// Starts a Yuanbao login and downloads the corresponding WeChat QR image.
pub async fn start_web_qr_login() -> Result<QrLoginSession> {
    let cookies = Arc::new(Jar::default());
    let client = login_client(Arc::clone(&cookies))?;
    let nonce = Uuid::new_v4().simple().to_string();
    let callback_url = callback_url(&nonce)?;
    let connect_url = qrconnect_url(&callback_url, now_millis()?)?;

    let response = client
        .get(connect_url.clone())
        .header(
            ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header(REFERER, YUANBAO_REFERER)
        .send()
        .await
        .map_err(|error| qr_network_error(&error))?;
    ensure_success(response.status(), "二维码页面")?;
    let page = read_body_limited(response, MAX_QR_PAGE_BYTES, qr_network_error).await?;
    let page = std::str::from_utf8(&page).map_err(|_| Error::UpstreamChanged)?;
    let uuid = qrcode_uuid_from_page(page).ok_or(Error::UpstreamChanged)?;

    let qrcode_url = Url::parse(&format!("{WECHAT_QR_IMAGE_ORIGIN}/connect/qrcode/{uuid}"))
        .map_err(|_| Error::UpstreamChanged)?;
    let response = client
        .get(qrcode_url.clone())
        .header(ACCEPT, "image/avif,image/webp,image/apng,image/*,*/*;q=0.8")
        .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header(REFERER, connect_url.as_str())
        .send()
        .await
        .map_err(|error| qr_network_error(&error))?;
    ensure_success(response.status(), "二维码图片")?;
    if response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| !value.starts_with("image/"))
    {
        return Err(Error::UpstreamChanged);
    }
    let qrcode_image = read_body_limited(response, MAX_QR_IMAGE_BYTES, qr_network_error).await?;
    if qrcode_image.is_empty() {
        return Err(Error::UpstreamChanged);
    }

    Ok(QrLoginSession {
        client,
        cookies,
        connect_url,
        callback_url,
        qrcode_url,
        qrcode_image,
        uuid,
        last_status: AtomicU16::new(NO_LAST_STATUS),
    })
}

/// Polls the current Yuanbao QR session once.
pub async fn poll_web_qr_login(session: &QrLoginSession) -> Result<QrPollStatus> {
    let mut endpoint =
        Url::parse(WECHAT_QR_POLL_URL).map_err(|_| Error::Config("微信扫码轮询地址无效".into()))?;
    {
        let mut query = endpoint.query_pairs_mut();
        query.append_pair("uuid", &session.uuid);
        let last = session.last_status.load(Ordering::Relaxed);
        if last != NO_LAST_STATUS {
            query.append_pair("last", &last.to_string());
        }
    }

    let response = session
        .client
        .get(endpoint)
        .header(ACCEPT, "*/*")
        .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header(REFERER, session.connect_url.as_str())
        .send()
        .await
        .map_err(|error| poll_network_error(&error))?;
    ensure_success(response.status(), "扫码状态轮询")?;
    let body = read_body_limited(response, MAX_POLL_BYTES, poll_network_error).await?;
    let body = std::str::from_utf8(&body).map_err(|_| Error::UpstreamChanged)?;
    let (status, code) = parse_poll_response(body)?;

    match status {
        408 => {
            session.last_status.store(NO_LAST_STATUS, Ordering::Relaxed);
            Ok(QrPollStatus::WaitingScan)
        }
        404 => {
            session.last_status.store(404, Ordering::Relaxed);
            Ok(QrPollStatus::WaitingConfirm)
        }
        403 => {
            session.last_status.store(403, Ordering::Relaxed);
            Ok(QrPollStatus::Cancelled)
        }
        402 => Ok(QrPollStatus::Expired),
        405 => {
            let code = code
                .filter(|value| !value.is_empty())
                .ok_or(Error::UpstreamChanged)?;
            let cookie = exchange_code_for_cookie(session, &code).await?;
            Ok(QrPollStatus::Success(cookie))
        }
        500 => Err(Error::Network("二维码状态异常，请重新执行登录".into())),
        _ => Err(Error::UpstreamChanged),
    }
}

/// Waits until a QR login succeeds, expires, or reaches the supplied timeout.
pub async fn wait_web_qr_login(
    session: &QrLoginSession,
    poll_interval: Duration,
    overall_timeout: Duration,
) -> Result<CookieCredential> {
    let deadline = tokio::time::Instant::now() + overall_timeout;
    let mut scanned = false;
    let mut cancelled = false;

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::Network("扫码登录超时".into()));
        }

        let status = tokio::time::timeout_at(deadline, poll_web_qr_login(session))
            .await
            .map_err(|_| Error::Network("扫码登录超时".into()))??;
        let delay = match status {
            QrPollStatus::WaitingScan => poll_interval,
            QrPollStatus::WaitingConfirm => {
                if !scanned {
                    tracing::info!("yuanbao qr: scanned, waiting for confirm");
                    scanned = true;
                }
                Duration::from_millis(100)
            }
            QrPollStatus::Cancelled => {
                if !cancelled {
                    tracing::info!("yuanbao qr: confirmation cancelled, waiting for another scan");
                    cancelled = true;
                }
                poll_interval
            }
            QrPollStatus::Expired => {
                return Err(Error::Network("二维码已过期，请重新执行登录".into()));
            }
            QrPollStatus::Success(cookie) => return Ok(cookie),
        };

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        tokio::time::sleep(delay.min(remaining)).await;
    }
}

#[derive(Serialize)]
struct JointLoginRequest<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(rename = "jsCode")]
    js_code: &'a str,
    appid: &'static str,
    #[serde(rename = "apiFeature")]
    api_feature: &'static str,
}

async fn exchange_code_for_cookie(
    session: &QrLoginSession,
    authorization_code: &str,
) -> Result<CookieCredential> {
    let endpoint =
        Url::parse(YUANBAO_LOGIN_URL).map_err(|_| Error::Config("元宝登录地址无效".into()))?;
    let response = session
        .client
        .post(endpoint)
        .header(ACCEPT, "application/json, text/plain, */*")
        .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
        .header(CONTENT_TYPE, "application/json")
        .header(ORIGIN, YUANBAO_ORIGIN)
        .header(REFERER, session.callback_url.as_str())
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header("sec-ch-ua", SEC_CH_UA_VALUE)
        .header("sec-ch-ua-mobile", "?0")
        .header("sec-ch-ua-platform", r#""macOS""#)
        .header("sec-fetch-dest", "empty")
        .header("sec-fetch-mode", "cors")
        .header("sec-fetch-site", "same-origin")
        .header("x-language", "zh-CN")
        .header("x-platform", "mac")
        .header("x-requested-with", "XMLHttpRequest")
        .header("x-source", "web")
        .header("x-web-third-source", "main")
        .header("x-webdriver", "0")
        .header("x-webversion", "2.80.0")
        .json(&JointLoginRequest {
            kind: "wx",
            js_code: authorization_code,
            appid: WECHAT_APP_ID,
            api_feature: "team",
        })
        .send()
        .await
        .map_err(|error| login_network_error(&error))?;
    ensure_success(response.status(), "元宝登录换票")?;
    let body = read_body_limited(response, MAX_LOGIN_BYTES, login_network_error).await?;
    let value: Value = serde_json::from_slice(&body).map_err(|_| Error::UpstreamChanged)?;

    if let Some(message) = login_response_error(&value) {
        return Err(Error::Network(message));
    }

    if let Some(cookie) = credential_from_jar(&session.cookies)? {
        return Ok(cookie);
    }

    let data = value.get("data").unwrap_or(&Value::Null);
    if data.get("registrered").and_then(Value::as_bool) == Some(false) {
        return Err(Error::Config(
            "扫码成功，但该微信尚未完成元宝账号注册或绑定；请先在元宝网页完成一次登录".into(),
        ));
    }
    if data
        .get("subToken")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        return Err(Error::Config(
            "扫码成功，但该账号需要在元宝网页选择登录身份".into(),
        ));
    }

    Err(Error::Network(
        "扫码成功但未拿到元宝登录 Cookie，请重试".into(),
    ))
}

fn login_client(cookies: Arc<Jar>) -> Result<Client> {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(35))
        .redirect(Policy::limited(4))
        .cookie_provider(cookies)
        .no_proxy()
        .build()
        .map_err(|_| Error::Config("无法初始化元宝扫码登录客户端".into()))
}

fn callback_url(nonce: &str) -> Result<Url> {
    let mut url =
        Url::parse(YUANBAO_SCAN_URL).map_err(|_| Error::Config("元宝扫码回调地址无效".into()))?;
    url.query_pairs_mut().append_pair("nonce", nonce);
    Ok(url)
}

fn qrconnect_url(callback_url: &Url, timestamp: u128) -> Result<Url> {
    let mut url = Url::parse(WECHAT_QRCONNECT_URL)
        .map_err(|_| Error::Config("微信扫码登录地址无效".into()))?;
    url.query_pairs_mut()
        .append_pair("appid", WECHAT_APP_ID)
        .append_pair("scope", "snsapi_login")
        .append_pair("redirect_uri", callback_url.as_str())
        .append_pair("state", "wechat_login")
        .append_pair("login_type", "jssdk")
        .append_pair("self_redirect", "false")
        .append_pair("style", "white")
        .append_pair("ts", &timestamp.to_string());
    Ok(url)
}

fn qrcode_uuid_from_page(page: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"/connect/qrcode/([0-9A-Za-z_-]{8,128})"#)
            .expect("constant QR image regex must compile")
    })
    .captures(page)
    .and_then(|captures| captures.get(1))
    .map(|value| value.as_str().to_owned())
}

fn parse_poll_response(body: &str) -> Result<(u16, Option<String>)> {
    static STATUS_RE: OnceLock<Regex> = OnceLock::new();
    static CODE_RE: OnceLock<Regex> = OnceLock::new();
    let status = STATUS_RE
        .get_or_init(|| {
            Regex::new(r#"wx_errcode\s*=\s*(\d+)"#)
                .expect("constant WeChat status regex must compile")
        })
        .captures(body)
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse::<u16>().ok())
        .ok_or(Error::UpstreamChanged)?;
    let code = CODE_RE
        .get_or_init(|| {
            Regex::new(r#"wx_code\s*=\s*['\"]([^'\"]*)['\"]"#)
                .expect("constant WeChat code regex must compile")
        })
        .captures(body)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned());
    Ok((status, code))
}

fn credential_from_jar(jar: &Jar) -> Result<Option<CookieCredential>> {
    let origin =
        Url::parse(YUANBAO_ORIGIN).map_err(|_| Error::Config("元宝站点地址无效".into()))?;
    let Some(header) = jar.cookies(&origin) else {
        return Ok(None);
    };
    let header = header.to_str().map_err(|_| Error::UpstreamChanged)?;
    if assess_yuanbao_cookie(header) != CredentialStatus::Present {
        return Ok(None);
    }
    Ok(CookieCredential::new(header.to_owned()))
}

fn login_response_error(value: &Value) -> Option<String> {
    let root_code = value.get("code").and_then(value_as_i64);
    if root_code.is_some_and(|code| code != 0) {
        return Some(
            response_message(value).unwrap_or_else(|| "元宝登录失败，请稍后重试".to_owned()),
        );
    }

    let error = value.get("error")?;
    if error.is_null() {
        return None;
    }
    let error_code = error.get("code").and_then(value_as_i64);
    if error_code.is_some_and(|code| code == 0) {
        return None;
    }
    let message = response_message(error);
    if error_code.is_none() && message.is_none() {
        return None;
    }
    Some(message.unwrap_or_else(|| "元宝登录失败，请稍后重试".to_owned()))
}

fn response_message(value: &Value) -> Option<String> {
    ["message", "msg", "errMsg"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_owned)
}

fn value_as_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.parse::<i64>().ok())
}

fn ensure_success(status: reqwest::StatusCode, action: &str) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(Error::RateLimited);
    }
    Err(Error::Network(format!(
        "{action}返回 HTTP {}",
        status.as_u16()
    )))
}

fn now_millis() -> Result<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .map_err(|_| Error::Config("系统时间早于 Unix epoch".into()))
}

fn qr_network_error(error: &reqwest::Error) -> Error {
    map_network_error(error, "二维码请求超时", "无法连接微信扫码登录服务")
}

fn poll_network_error(error: &reqwest::Error) -> Error {
    map_network_error(error, "扫码状态轮询超时", "无法连接微信扫码登录服务")
}

fn login_network_error(error: &reqwest::Error) -> Error {
    map_network_error(error, "元宝登录换票超时", "无法完成元宝登录换票")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_qrcode_uuid_from_wechat_page() {
        let page = r#"<img class="qrcode" src="/connect/qrcode/041DnvaV3L8MGa1e">"#;
        assert_eq!(
            qrcode_uuid_from_page(page).as_deref(),
            Some("041DnvaV3L8MGa1e")
        );
    }

    #[test]
    fn builds_qrconnect_callback_with_nonce() {
        let callback = callback_url("nonce-123").unwrap();
        let url = qrconnect_url(&callback, 42).unwrap();
        assert_eq!(url.host_str(), Some("open.weixin.qq.com"));
        let query = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            query.get("appid").map(|value| value.as_ref()),
            Some(WECHAT_APP_ID)
        );
        assert_eq!(
            query.get("state").map(|value| value.as_ref()),
            Some("wechat_login")
        );
        assert_eq!(
            query.get("redirect_uri").map(|value| value.as_ref()),
            Some("https://yuanbao.tencent.com/scan?nonce=nonce-123")
        );
    }

    #[test]
    fn parses_wechat_poll_javascript() {
        assert_eq!(
            parse_poll_response("window.wx_errcode=408;window.wx_code='';").unwrap(),
            (408, Some(String::new()))
        );
        assert_eq!(
            parse_poll_response("window.wx_errcode = 405; window.wx_code = \"code_123\";").unwrap(),
            (405, Some("code_123".to_owned()))
        );
        assert!(parse_poll_response("not javascript").is_err());
    }

    #[test]
    fn reads_yuanbao_cookie_from_jar() {
        let jar = Jar::default();
        let origin = Url::parse(YUANBAO_ORIGIN).unwrap();
        jar.add_cookie_str("hy_user=user-id; Path=/; Secure", &origin);
        jar.add_cookie_str("hy_token=secret; Path=/; Secure; HttpOnly", &origin);
        let cookie = credential_from_jar(&jar).unwrap().unwrap();
        assert!(cookie.as_str().contains("hy_user=user-id"));
        assert!(cookie.as_str().contains("hy_token=secret"));
    }

    #[test]
    fn maps_wrapped_login_errors_without_dumping_payloads() {
        let value = serde_json::json!({
            "error": {"code": "400", "message": "登录码无效"}
        });
        assert_eq!(login_response_error(&value).as_deref(), Some("登录码无效"));
        assert!(login_response_error(&serde_json::json!({"code": 0, "data": {}})).is_none());
        assert!(login_response_error(&serde_json::json!({"error": null})).is_none());
    }
}
