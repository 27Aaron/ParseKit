//! Best-effort `Content-Length` probing when parsers omit `size_hint`.

use std::{collections::HashSet, time::Duration};

use reqwest::{
    Client, Method, StatusCode,
    header::{ACCEPT, ACCEPT_ENCODING, CONTENT_LENGTH, ORIGIN, RANGE, REFERER, USER_AGENT},
    redirect::Policy,
};
use tokio::time::timeout;
use url::Url;

use crate::{ResolvedPost, Result, media::DownloadRequestIdentity, platforms};

use super::CONNECT_TIMEOUT;
use super::ssrf::{normalize_allowed_hosts, resolve_public_addresses, validate_media_url};

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_REDIRECTS: usize = 5;
const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// Fills missing [`crate::MediaSource::size_hint`] via lightweight HTTP probes.
///
/// Soft-fail: probe errors leave the field unset and do not abort resolve.
pub async fn enrich_missing_size_hints(post: &mut ResolvedPost) {
    let identity = DownloadRequestIdentity::for_platform(post.platform);
    let hosts = platforms::platform_spec(post.platform).reviewed_media_hosts();
    let Ok(allowed) = normalize_allowed_hosts(hosts.iter().copied()) else {
        return;
    };

    let missing: Vec<(usize, Url)> = post
        .media_sources()
        .enumerate()
        .filter(|(_, source)| source.size_hint.is_none())
        .map(|(index, source)| (index, source.url.clone()))
        .collect();

    for (index, url) in missing {
        let size = match timeout(
            PROBE_TIMEOUT,
            probe_content_length(&url, &identity, &allowed),
        )
        .await
        {
            Ok(Ok(size)) => size,
            _ => None,
        };
        if let Some(bytes) = size
            && let Some(source) = post.media_source_at_mut(index)
        {
            source.size_hint = Some(bytes);
        }
    }
}

async fn probe_content_length(
    url: &Url,
    identity: &DownloadRequestIdentity,
    allowed: &HashSet<String>,
) -> Result<Option<u64>> {
    let mut current = url.clone();
    for _ in 0..=MAX_REDIRECTS {
        let host = validate_media_url(&current, allowed)?.to_ascii_lowercase();
        let port = current.port_or_known_default().unwrap_or(443);
        let addresses = resolve_public_addresses(&host, port).await?;
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(PROBE_TIMEOUT)
            .https_only(true)
            .no_proxy()
            .resolve_to_addrs(&host, &addresses)
            .build()
            .map_err(|_| crate::Error::Config("无法初始化体积探测 HTTP 客户端".into()))?;

        // HEAD first (no body).
        let head = request_raw(&client, Method::HEAD, &current, identity, false).await?;
        if head.status().is_redirection() {
            if let Some(next) = redirect_location(&head, &current) {
                current = next;
                continue;
            }
            return Ok(None);
        }
        if let Some(len) = size_from_response(&head, false) {
            return Ok(Some(len));
        }

        // Many CDNs only answer with Content-Range on a ranged GET.
        let ranged = request_raw(&client, Method::GET, &current, identity, true).await?;
        if ranged.status().is_redirection() {
            if let Some(next) = redirect_location(&ranged, &current) {
                current = next;
                continue;
            }
            return Ok(None);
        }
        if let Some(len) = size_from_response(&ranged, true) {
            return Ok(Some(len));
        }
        return Ok(None);
    }
    Ok(None)
}

fn size_from_response(response: &reqwest::Response, ranged: bool) -> Option<u64> {
    let status = response.status();
    if status != StatusCode::OK && status != StatusCode::PARTIAL_CONTENT {
        return None;
    }
    if let Some(total) = content_range_total(response.headers()) {
        return Some(total);
    }
    let len = content_length_header(response.headers())?;
    // Range responses often report Content-Length: 1 (the slice), not the file.
    if ranged && len <= 1 {
        return None;
    }
    Some(len)
}

async fn request_raw(
    client: &Client,
    method: Method,
    url: &Url,
    identity: &DownloadRequestIdentity,
    range: bool,
) -> Result<reqwest::Response> {
    let mut request = client
        .request(method, url.clone())
        .header(ACCEPT, "*/*")
        .header(ACCEPT_ENCODING, "identity");
    if range {
        request = request.header(RANGE, "bytes=0-0");
    }
    if let Some(origin) = identity.origin.as_deref() {
        request = request.header(ORIGIN, origin);
    }
    if let Some(referer) = identity.referer.as_deref() {
        request = request.header(REFERER, referer);
    }
    let ua = identity.user_agent.as_deref().unwrap_or(DEFAULT_UA);
    request = request.header(USER_AGENT, ua);
    request
        .send()
        .await
        .map_err(|_| crate::Error::Network("体积探测请求失败".into()))
}

fn content_length_header(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers.get(CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
}

/// Parses `Content-Range: bytes 0-0/123456` → `123456`.
pub(super) fn content_range_total(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let raw = headers.get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?;
    let total = raw.rsplit('/').next()?;
    if total == "*" {
        return None;
    }
    total.parse().ok()
}

fn redirect_location(response: &reqwest::Response, current: &Url) -> Option<Url> {
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)?
        .to_str()
        .ok()?;
    current.join(location).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    #[test]
    fn parses_content_range_total() {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 0-0/5816439"),
        );
        assert_eq!(content_range_total(&headers), Some(5_816_439));
    }

    #[test]
    fn rejects_star_content_range() {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 0-0/*"),
        );
        assert_eq!(content_range_total(&headers), None);
    }
}
