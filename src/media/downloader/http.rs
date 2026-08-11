//! HTTP client pinning, response validation, and transient retries.

use std::{future::Future, net::SocketAddr, time::Duration};

use reqwest::{
    Client, Response, StatusCode,
    header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, HeaderMap},
    redirect::Policy,
};
use tracing::warn;

use crate::{Error, Result};

use super::CONNECT_TIMEOUT;

pub(super) async fn retry_transient_downloads<T, F, Fut>(
    mut operation: F,
    retry_delays: &[Duration],
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let max_attempts = retry_delays.len() + 1;
    for attempt in 1..=max_attempts {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) if is_transient_download_error(&error) && attempt < max_attempts => {
                let delay = retry_delays[attempt - 1];
                warn!(
                    event = "media_download_retry",
                    attempt,
                    max_attempts,
                    ?delay,
                    error = %error,
                    "媒体下载遇到临时错误，准备重试"
                );
                tokio::time::sleep(delay).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the download retry loop always returns on its final attempt")
}

pub(super) fn is_transient_download_error(error: &Error) -> bool {
    // Retry rate limits on the fixed transient schedule.
    matches!(error, Error::Network(_) | Error::RateLimited)
}

pub(super) fn pinned_http_client(
    host: &str,
    addresses: &[SocketAddr],
    request_timeout: Duration,
) -> Result<Client> {
    Client::builder()
        .redirect(Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(request_timeout)
        .https_only(true)
        .no_proxy()
        .resolve_to_addrs(host, addresses)
        .build()
        .map_err(|_| Error::Config("无法初始化媒体 HTTP 客户端".to_owned()))
}

pub(super) fn check_response_status(status: StatusCode) -> Result<()> {
    match status {
        StatusCode::OK => Ok(()),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Error::Expired),
        StatusCode::NOT_FOUND | StatusCode::GONE => Err(Error::NotFound),
        StatusCode::TOO_MANY_REQUESTS => Err(Error::RateLimited),
        StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_EARLY => Err(Error::Network(format!(
            "媒体服务器暂时不可用（HTTP {}）",
            status.as_u16()
        ))),
        status if status.is_server_error() => Err(Error::Network(format!(
            "媒体服务器暂时不可用（HTTP {}）",
            status.as_u16()
        ))),
        status => Err(Error::Download(format!(
            "媒体服务器返回 HTTP {}",
            status.as_u16()
        ))),
    }
}

pub(super) fn checked_content_length(response: &Response) -> Result<Option<u64>> {
    let Some(raw) = response.headers().get(CONTENT_LENGTH) else {
        return Ok(None);
    };
    let raw = raw
        .to_str()
        .map_err(|_| Error::Download("媒体 Content-Length 无效".to_owned()))?;
    let length = raw
        .parse::<u64>()
        .map_err(|_| Error::Download("媒体 Content-Length 无效".to_owned()))?;
    Ok(Some(length))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ContentRange {
    pub(super) start: Option<u64>,
    pub(super) end: Option<u64>,
    pub(super) total: Option<u64>,
}

impl ContentRange {
    pub(super) fn response_length(self) -> Option<u64> {
        self.end?.checked_sub(self.start?)?.checked_add(1)
    }
}

/// Parses satisfied and unsatisfied byte `Content-Range` values.
pub(super) fn parse_content_range(headers: &HeaderMap) -> Option<ContentRange> {
    let raw = headers.get(CONTENT_RANGE)?.to_str().ok()?.trim();
    let (unit, value) = raw.split_once(' ')?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (range, total) = value.trim().split_once('/')?;
    let total = match total.trim() {
        "*" => None,
        value => Some(value.parse::<u64>().ok()?),
    };

    if range.trim() == "*" {
        return total.map(|total| ContentRange {
            start: None,
            end: None,
            total: Some(total),
        });
    }

    let (start, end) = range.trim().split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    if start > end
        || end.checked_sub(start)?.checked_add(1).is_none()
        || total.is_some_and(|total| end >= total)
    {
        return None;
    }
    Some(ContentRange {
        start: Some(start),
        end: Some(end),
        total,
    })
}

pub(super) fn reject_encoded_response(response: &Response) -> Result<()> {
    if let Some(value) = response.headers().get(CONTENT_ENCODING) {
        let value = value
            .to_str()
            .map_err(|_| Error::Download("媒体 Content-Encoding 无效".to_owned()))?;
        if !value.eq_ignore_ascii_case("identity") {
            return Err(Error::Download(
                "媒体服务器忽略了 identity 编码要求".to_owned(),
            ));
        }
    }
    Ok(())
}

pub(super) fn map_reqwest_download_error(error: reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::Network("媒体请求超时".to_owned())
    } else {
        Error::Network("媒体网络请求失败".to_owned())
    }
}
