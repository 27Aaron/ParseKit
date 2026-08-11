//! Yuanbao and WeChat API payload, cookie, and response helpers.

use reqwest::{Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Error, Result,
    auth::cookie_value as shared_cookie_value,
    platforms::util::{map_network_error as map_network_error_msg, read_body_limited},
};

const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;

/// Cookie lookup; prefers non-empty values (matches historical WeChat behavior).
pub(super) fn cookie_value(cookie: &str, name: &str) -> Option<String> {
    shared_cookie_value(cookie, name).filter(|value| !value.is_empty())
}

pub(super) fn text_at(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(value_to_string).and_then(non_empty)
}

pub(super) fn number_at(value: &Value, key: &str) -> Option<u64> {
    let value = value.get(key)?;
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse::<u64>().ok())
}

pub(super) fn integer_at(value: &Value, key: &str) -> Option<i64> {
    let value = value.get(key)?;
    value
        .as_i64()
        .or_else(|| value.as_str()?.parse::<i64>().ok())
}

pub(super) fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

pub(super) fn value_to_text(value: Option<&Value>) -> String {
    value
        .and_then(value_to_string)
        .unwrap_or_default()
        .to_ascii_lowercase()
}

pub(super) fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else if trimmed.len() == value.len() {
        Some(value)
    } else {
        Some(trimmed.to_owned())
    }
}

pub(super) fn response_looks_like_login(value: &Value) -> bool {
    ["msg", "errMsg", "message"]
        .into_iter()
        .map(|key| value_to_text(value.get(key)))
        .any(|message| {
            ["login", "登录", "cookie", "unauthorized", "未登录"]
                .iter()
                .any(|word| message.contains(word))
        })
}

pub(super) fn map_status(status: StatusCode, yuanbao: bool) -> Result<()> {
    match status {
        status if status.is_success() => Ok(()),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN if yuanbao => Err(Error::LoginRequired),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Error::NotFound),
        StatusCode::NOT_FOUND | StatusCode::GONE => Err(Error::NotFound),
        StatusCode::TOO_MANY_REQUESTS => Err(Error::RateLimited),
        _ => Err(Error::Network(format!("上游返回 HTTP {}", status.as_u16()))),
    }
}

pub(super) fn map_network_error(error: &reqwest::Error) -> Error {
    map_network_error_msg(error, "上游请求超时", "无法连接上游服务")
}

pub(super) async fn read_json(response: Response) -> Result<Value> {
    let bytes = read_body_limited(response, MAX_JSON_BYTES, map_network_error).await?;
    serde_json::from_slice(&bytes).map_err(|_| Error::UpstreamChanged)
}

#[derive(Serialize)]
pub(super) struct ParseRequest<'a> {
    #[serde(rename = "type")]
    pub(super) kind: &'a str,
    pub(super) url: &'a str,
    pub(super) scene: u8,
}

#[derive(Debug, Deserialize)]
pub(super) struct ParseData {
    #[serde(default)]
    pub(super) wx_export_id: String,
    #[serde(default)]
    pub(super) cover_url: String,
    #[serde(default)]
    pub(super) desc: String,
    pub(super) playable_url: String,
}

#[derive(Serialize)]
pub(super) struct FeedRequest<'a> {
    #[serde(rename = "baseReq")]
    pub(super) base_req: FeedBaseRequest<'a>,
    #[serde(rename = "exportId")]
    pub(super) export_id: &'a str,
}

#[derive(Serialize)]
pub(super) struct FeedBaseRequest<'a> {
    #[serde(rename = "generalToken")]
    pub(super) general_token: &'a str,
}
