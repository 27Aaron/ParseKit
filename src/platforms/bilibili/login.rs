//! Bilibili web QR login (passport API), aligned with BBDown's WEB flow.
//!
//! Success may deliver `SESSDATA` either:
//! - in the poll JSON success URL query (legacy BBDown path), or
//! - via `Set-Cookie` on the poll response / after GET of the success URL
//!   (current passport flow returns `ticket` + `gourl` and sets cookies on fetch).

use std::{sync::OnceLock, time::Duration};

use reqwest::{
    Client,
    header::{ACCEPT, ACCEPT_LANGUAGE, HeaderMap, LOCATION, REFERER, SET_COOKIE, USER_AGENT},
    redirect::Policy,
};
use serde_json::Value;
use url::Url;

use crate::{
    Error, Result,
    auth::{CookieCredential, cookie_value, query_string_to_cookie_header},
    platforms::util::{map_network_error, read_body_limited},
};

use super::hosts::USER_AGENT_VALUE;

const GENERATE_URL: &str =
    "https://passport.bilibili.com/x/passport-login/web/qrcode/generate?source=main-fe-header";
const POLL_URL: &str = "https://passport.bilibili.com/x/passport-login/web/qrcode/poll";
const MAX_JSON_BYTES: usize = 64 * 1024;
const MAX_COOKIE_EXCHANGE_REDIRECTS: usize = 8;

/// Cookie names worth keeping from passport / bilibili domain responses.
const KEEP_COOKIE_NAMES: &[&str] = &[
    "SESSDATA",
    "bili_jct",
    "DedeUserID",
    "DedeUserID__ckMd5",
    "sid",
    "bili_ticket",
    "bili_ticket_expires",
];

/// Started web QR session: open [`Self::url`] in the Bilibili app / browser.
#[derive(Debug, Clone)]
pub struct QrLoginSession {
    /// Full URL encoded in the QR code.
    pub url: String,
    /// Key used to poll login status.
    pub qrcode_key: String,
}

/// One poll of the QR login status endpoint.
#[derive(Debug, Clone)]
pub enum QrPollStatus {
    /// Waiting for the user to scan the code.
    WaitingScan,
    /// Scanned; waiting for confirmation in the app.
    WaitingConfirm,
    /// QR expired; start a new session.
    Expired,
    /// Login succeeded; cookie header ready for API requests.
    Success(CookieCredential),
}

/// Generates a web QR login session.
pub async fn start_web_qr_login() -> Result<QrLoginSession> {
    let client = passport_client()?;
    let response = client
        .get(GENERATE_URL)
        .header(ACCEPT, "application/json")
        .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9")
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header(REFERER, "https://www.bilibili.com/")
        .send()
        .await
        .map_err(|error| map_network_error(&error, "登录请求超时", "无法连接哔哩哔哩登录服务"))?;

    if !response.status().is_success() {
        return Err(Error::Network(format!(
            "登录接口返回 HTTP {}",
            response.status().as_u16()
        )));
    }

    let bytes = read_body_limited(response, MAX_JSON_BYTES, |error| {
        map_network_error(error, "登录请求超时", "无法连接哔哩哔哩登录服务")
    })
    .await?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::UpstreamChanged)?;
    if value.get("code").and_then(Value::as_i64).unwrap_or(-1) != 0 {
        return Err(Error::UpstreamChanged);
    }
    let data = value.get("data").ok_or(Error::UpstreamChanged)?;
    let url = data
        .get("url")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 4_096)
        .ok_or(Error::UpstreamChanged)?
        .to_owned();
    let parsed_url = Url::parse(&url).map_err(|_| Error::UpstreamChanged)?;
    if !is_allowed_bilibili_url(&parsed_url) {
        return Err(Error::UpstreamChanged);
    }
    let qrcode_key = data
        .get("qrcode_key")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| qrcode_key_from_url(&url))
        .ok_or(Error::UpstreamChanged)?;
    if !valid_qrcode_key(&qrcode_key) {
        return Err(Error::UpstreamChanged);
    }

    Ok(QrLoginSession { url, qrcode_key })
}

/// Polls once for QR login completion.
pub async fn poll_web_qr_login(qrcode_key: &str) -> Result<QrPollStatus> {
    let qrcode_key = qrcode_key.trim();
    if !valid_qrcode_key(qrcode_key) {
        return Err(Error::Config("qrcode_key 格式无效".into()));
    }
    let client = passport_client()?;
    let mut endpoint =
        Url::parse(POLL_URL).map_err(|_| Error::Config("哔哩哔哩 poll API 地址无效".into()))?;
    endpoint
        .query_pairs_mut()
        .append_pair("qrcode_key", qrcode_key)
        .append_pair("source", "main-fe-header");

    let response = client
        .get(endpoint)
        .header(ACCEPT, "application/json")
        .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9")
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header(REFERER, "https://www.bilibili.com/")
        .send()
        .await
        .map_err(|error| map_network_error(&error, "登录轮询超时", "无法连接哔哩哔哩登录服务"))?;

    if !response.status().is_success() {
        return Err(Error::Network(format!(
            "登录轮询返回 HTTP {}",
            response.status().as_u16()
        )));
    }

    // Prefer Set-Cookie from the poll response (some clients get SESSDATA here).
    let mut pairs = set_cookie_pairs(response.headers());

    let bytes = read_body_limited(response, MAX_JSON_BYTES, |error| {
        map_network_error(error, "登录轮询超时", "无法连接哔哩哔哩登录服务")
    })
    .await?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::UpstreamChanged)?;
    let data = value.get("data").ok_or(Error::UpstreamChanged)?;
    let code = data.get("code").and_then(Value::as_i64).unwrap_or(-1);

    match code {
        86101 => Ok(QrPollStatus::WaitingScan),
        86090 => Ok(QrPollStatus::WaitingConfirm),
        86038 => Ok(QrPollStatus::Expired),
        0 => {
            let success_url = data
                .get("url")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty() && s.len() <= 4_096)
                .ok_or(Error::UpstreamChanged)?;
            let parsed_success_url = Url::parse(success_url).map_err(|_| Error::UpstreamChanged)?;
            if !is_allowed_bilibili_url(&parsed_success_url) {
                return Err(Error::UpstreamChanged);
            }

            // Legacy BBDown: SESSDATA embedded in the success URL query string.
            merge_query_session_pairs(success_url, &mut pairs);

            // Current flow: success URL has ticket/gourl; GET it to receive Set-Cookie.
            if !has_sessdata(&pairs) {
                exchange_success_url_for_cookies(&client, success_url, &mut pairs).await?;
            }

            let cookie = credential_from_pairs(&pairs)?;
            Ok(QrPollStatus::Success(cookie))
        }
        _ => Err(Error::UpstreamChanged),
    }
}

/// Blocks until QR login succeeds, expires, or the optional timeout elapses.
pub async fn wait_web_qr_login(
    qrcode_key: &str,
    poll_interval: Duration,
    overall_timeout: Duration,
) -> Result<CookieCredential> {
    let deadline = tokio::time::Instant::now()
        .checked_add(overall_timeout)
        .ok_or_else(|| Error::Config("扫码登录超时时间过大".into()))?;
    let mut confirmed = false;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::Network("扫码登录超时".into()));
        }
        match tokio::time::timeout_at(deadline, poll_web_qr_login(qrcode_key))
            .await
            .map_err(|_| Error::Network("扫码登录超时".into()))??
        {
            QrPollStatus::WaitingScan => {}
            QrPollStatus::WaitingConfirm => {
                if !confirmed {
                    tracing::info!("bilibili qr: scanned, waiting for confirm");
                    confirmed = true;
                }
            }
            QrPollStatus::Expired => {
                return Err(Error::Network("二维码已过期，请重新执行登录".into()));
            }
            QrPollStatus::Success(cookie) => return Ok(cookie),
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        tokio::time::sleep(poll_interval.min(remaining)).await;
    }
}

/// GET the passport success URL (and redirects) and merge session Set-Cookie values.
async fn exchange_success_url_for_cookies(
    client: &Client,
    success_url: &str,
    pairs: &mut Vec<(String, String)>,
) -> Result<()> {
    let mut current = Url::parse(success_url).map_err(|_| Error::UpstreamChanged)?;
    for _ in 0..MAX_COOKIE_EXCHANGE_REDIRECTS {
        if !is_allowed_bilibili_url(&current) {
            return Err(Error::UpstreamChanged);
        }
        let response = client
            .get(current.clone())
            .header(ACCEPT, "*/*")
            .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .header(REFERER, "https://www.bilibili.com/")
            .send()
            .await
            .map_err(|error| {
                map_network_error(&error, "登录换票超时", "无法完成哔哩哔哩登录换票")
            })?;

        merge_pairs(pairs, set_cookie_pairs(response.headers()));
        if has_sessdata(pairs) {
            return Ok(());
        }

        if !response.status().is_redirection() {
            // Final page may still have set cookies even without SESSDATA if upstream changed.
            return Ok(());
        }
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or(Error::UpstreamChanged)?;
        current = current.join(location).map_err(|_| Error::UpstreamChanged)?;
    }
    Err(Error::UpstreamChanged)
}

fn merge_query_session_pairs(success_url: &str, pairs: &mut Vec<(String, String)>) {
    let Ok(url) = Url::parse(success_url) else {
        return;
    };
    let Some(query) = url.query() else {
        return;
    };
    // Only pull known session keys from the query — not ticket/gourl.
    for (name, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if KEEP_COOKIE_NAMES
            .iter()
            .any(|keep| keep.eq_ignore_ascii_case(&name))
        {
            upsert_pair(pairs, name.into_owned(), value.into_owned());
        }
    }
    // Also accept BBDown-style full cookie dump if the query already looks like one.
    let as_header = query_string_to_cookie_header(query);
    if cookie_value(&as_header, "SESSDATA").is_some() {
        for part in as_header.split(';') {
            let part = part.trim();
            if let Some((name, value)) = part.split_once('=')
                && KEEP_COOKIE_NAMES
                    .iter()
                    .any(|keep| keep.eq_ignore_ascii_case(name))
            {
                upsert_pair(pairs, name.to_owned(), value.to_owned());
            }
        }
    }
}

fn set_cookie_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for value in headers.get_all(SET_COOKIE) {
        let Ok(raw) = value.to_str() else {
            continue;
        };
        // `name=value; Path=/; Domain=...`
        let first = raw.split(';').next().unwrap_or("").trim();
        let Some((name, val)) = first.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        // Keep known session cookies; also keep SESSDATA even if list drifts.
        if KEEP_COOKIE_NAMES
            .iter()
            .any(|keep| keep.eq_ignore_ascii_case(name))
            || name.eq_ignore_ascii_case("SESSDATA")
        {
            upsert_pair(&mut out, name.to_owned(), val.to_owned());
        }
    }
    out
}

fn merge_pairs(into: &mut Vec<(String, String)>, from: Vec<(String, String)>) {
    for (name, value) in from {
        upsert_pair(into, name, value);
    }
}

fn upsert_pair(pairs: &mut Vec<(String, String)>, name: String, value: String) {
    if let Some((_, existing)) = pairs
        .iter_mut()
        .find(|(n, _)| n.eq_ignore_ascii_case(&name))
    {
        *existing = value;
    } else {
        pairs.push((name, value));
    }
}

fn has_sessdata(pairs: &[(String, String)]) -> bool {
    pairs
        .iter()
        .any(|(n, v)| n.eq_ignore_ascii_case("SESSDATA") && !v.is_empty())
}

fn credential_from_pairs(pairs: &[(String, String)]) -> Result<CookieCredential> {
    if !has_sessdata(pairs) {
        return Err(Error::Network(
            "登录成功但未拿到 SESSDATA（cookie 交换失败，请重试）".into(),
        ));
    }
    if pairs.iter().any(|(name, value)| {
        name.is_empty()
            || name.len() > 128
            || value.len() > 8_192
            || name
                .bytes()
                .chain(value.bytes())
                .any(|byte| byte.is_ascii_control() || byte == b';')
    }) {
        return Err(Error::UpstreamChanged);
    }
    let header = pairs
        .iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    CookieCredential::new(header).ok_or(Error::UpstreamChanged)
}

fn qrcode_key_from_url(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "qrcode_key")
        .map(|(_, v)| v.into_owned())
}

fn valid_qrcode_key(key: &str) -> bool {
    (1..=256).contains(&key.len())
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

/// Passport controls this URL, but validate every hop before requesting it so
/// an upstream payload change cannot turn the cookie exchange into an SSRF.
fn is_allowed_bilibili_url(url: &Url) -> bool {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
    {
        return false;
    }
    url.host_str().is_some_and(|host| {
        !host.ends_with('.')
            && (host.eq_ignore_ascii_case("bilibili.com")
                || host
                    .to_ascii_lowercase()
                    .strip_suffix(".bilibili.com")
                    .is_some_and(|prefix| !prefix.is_empty()))
    })
}

fn passport_client() -> Result<Client> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        // Follow redirects ourselves so every hop's Set-Cookie is visible.
        .redirect(Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| Error::Config("无法初始化哔哩哔哩登录 HTTP 客户端".into()))?;
    Ok(CLIENT.get_or_init(|| client).clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn cookie_from_legacy_success_url_query() {
        let mut pairs = Vec::new();
        let url = "https://passport.bilibili.com/x/passport-login/web/crossDomain?DedeUserID=1&SESSDATA=tok%2Cen&bili_jct=jct&gourl=https%3A%2F%2Fwww.bilibili.com";
        merge_query_session_pairs(url, &mut pairs);
        assert!(has_sessdata(&pairs));
        let cookie = credential_from_pairs(&pairs).unwrap();
        assert!(cookie.as_str().contains("SESSDATA=tok%2Cen"));
        assert!(cookie.as_str().contains("bili_jct=jct"));
        // gourl / ticket must not be stored as cookies
        assert!(!cookie.as_str().contains("gourl="));
    }

    #[test]
    fn ticket_only_query_is_not_treated_as_session() {
        let mut pairs = Vec::new();
        let url = "https://passport.bilibili.com/x/passport-login/web/crossDomain?ticket=abc&gourl=https%3A%2F%2Fwww.bilibili.com&first_domain=.bilibili.com";
        merge_query_session_pairs(url, &mut pairs);
        assert!(!has_sessdata(&pairs));
        assert!(credential_from_pairs(&pairs).is_err());
    }

    #[test]
    fn set_cookie_headers_yield_sessdata() {
        let mut headers = HeaderMap::new();
        headers.append(
            SET_COOKIE,
            HeaderValue::from_static("SESSDATA=secret; Path=/; Domain=.bilibili.com; HttpOnly"),
        );
        headers.append(
            SET_COOKIE,
            HeaderValue::from_static("bili_jct=csrf; Path=/; Domain=.bilibili.com"),
        );
        headers.append(
            SET_COOKIE,
            HeaderValue::from_static("noise=1; Path=/"), // ignored
        );
        let pairs = set_cookie_pairs(&headers);
        assert!(has_sessdata(&pairs));
        let cookie = credential_from_pairs(&pairs).unwrap();
        assert!(cookie.as_str().contains("SESSDATA=secret"));
        assert!(cookie.as_str().contains("bili_jct=csrf"));
        assert!(!cookie.as_str().contains("noise="));
    }

    #[test]
    fn cookie_exchange_urls_are_restricted_to_bilibili_https() {
        for raw in [
            "https://passport.bilibili.com/x/passport-login/web/crossDomain",
            "https://www.bilibili.com/",
            "https://bilibili.com/",
        ] {
            assert!(is_allowed_bilibili_url(&Url::parse(raw).unwrap()), "{raw}");
        }
        for raw in [
            "http://passport.bilibili.com/x",
            "https://passport.bilibili.com.evil.test/x",
            "https://user@passport.bilibili.com/x",
            "https://passport.bilibili.com:8443/x",
        ] {
            assert!(!is_allowed_bilibili_url(&Url::parse(raw).unwrap()), "{raw}");
        }
    }

    #[test]
    fn qrcode_keys_are_bounded_and_header_safe() {
        assert!(valid_qrcode_key("abc_DEF-123"));
        assert!(!valid_qrcode_key(""));
        assert!(!valid_qrcode_key("contains space"));
        assert!(!valid_qrcode_key(&"a".repeat(257)));
    }

    #[test]
    fn cookie_pair_serialization_rejects_delimiter_injection() {
        let pairs = vec![("SESSDATA".to_owned(), "secret; injected=1".to_owned())];
        assert!(credential_from_pairs(&pairs).is_err());
    }
}
