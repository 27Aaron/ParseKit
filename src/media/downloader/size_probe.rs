//! Best-effort `Content-Length` probing when parsers omit `size_hint`.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use reqwest::{
    Client, Method, StatusCode,
    header::{ACCEPT, ACCEPT_ENCODING, CONTENT_LENGTH, ORIGIN, RANGE, REFERER, USER_AGENT},
    redirect::Policy,
};
use tokio::{sync::Semaphore, task::JoinSet, time::timeout_at};
use url::Url;

use crate::{ResolvedPost, Result, media::DownloadRequestIdentity, platforms};

use super::CONNECT_TIMEOUT;
use super::http::parse_content_range;
use super::ssrf::{normalize_allowed_hosts, resolve_public_addresses, validate_media_url};

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_CONCURRENT_PROBES: usize = 4;
const MAX_REDIRECTS: usize = 5;
const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// Best-effort population of missing [`crate::MediaSource::size_hint`] values.
pub async fn enrich_missing_size_hints(post: &mut ResolvedPost) {
    let identity = DownloadRequestIdentity::for_platform(post.platform);
    let hosts = platforms::platform_spec(post.platform).reviewed_media_hosts();
    let Ok(allowed) = normalize_allowed_hosts(hosts.iter().copied()) else {
        return;
    };

    // Probe each unique CDN URL once.
    let mut missing = Vec::<(Url, Vec<usize>)>::new();
    let mut positions = HashMap::<Url, usize>::new();
    for (index, source) in post
        .media_sources()
        .enumerate()
        .filter(|(_, source)| source.size_hint.is_none())
    {
        if let Some(position) = positions.get(&source.url).copied() {
            missing[position].1.push(index);
        } else {
            positions.insert(source.url.clone(), missing.len());
            missing.push((source.url.clone(), vec![index]));
        }
    }

    let identity = Arc::new(identity);
    let allowed = Arc::new(allowed);
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_PROBES));
    let mut probes = JoinSet::new();
    for (url, indices) in missing {
        let identity = Arc::clone(&identity);
        let allowed = Arc::clone(&allowed);
        let permits = Arc::clone(&permits);
        probes.spawn(async move {
            let Ok(_permit) = permits.acquire_owned().await else {
                return (indices, None);
            };
            let size = probe_content_length(&url, &identity, &allowed)
                .await
                .ok()
                .flatten();
            (indices, size)
        });
    }

    // Bound the entire optional enrichment phase.
    let deadline = tokio::time::Instant::now() + PROBE_TIMEOUT;
    loop {
        match timeout_at(deadline, probes.join_next()).await {
            Ok(Some(Ok((indices, Some(bytes))))) => {
                for index in indices {
                    if let Some(source) = post.media_source_at_mut(index) {
                        source.size_hint = Some(bytes);
                    }
                }
            }
            Ok(Some(Ok((_, None)) | Err(_))) => {}
            Ok(None) => break,
            Err(_) => {
                probes.abort_all();
                break;
            }
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

        let head = request_raw(&client, Method::HEAD, &current, identity, false).await?;
        if head.status().is_redirection() {
            if let Some(next) = redirect_location(&head, &current) {
                current = next;
                continue;
            }
            return Ok(None);
        }
        if let Some(len) = size_from_response(&head) {
            return Ok(Some(len));
        }

        let ranged = request_raw(&client, Method::GET, &current, identity, true).await?;
        if ranged.status().is_redirection() {
            if let Some(next) = redirect_location(&ranged, &current) {
                current = next;
                continue;
            }
            return Ok(None);
        }
        if let Some(len) = size_from_response(&ranged) {
            return Ok(Some(len));
        }
        return Ok(None);
    }
    Ok(None)
}

fn size_from_response(response: &reqwest::Response) -> Option<u64> {
    let status = response.status();
    if status != StatusCode::OK && status != StatusCode::PARTIAL_CONTENT {
        return None;
    }
    if let Some(total) = content_range_total(response.headers()).filter(|total| *total > 0) {
        return Some(total);
    }
    // A 206 Content-Length covers only the returned range.
    if status == StatusCode::PARTIAL_CONTENT {
        return None;
    }
    content_length_header(response.headers()).filter(|len| *len > 0)
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

/// Returns the total length from `Content-Range`.
pub(super) fn content_range_total(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    parse_content_range(headers)?.total
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

    #[test]
    fn rejects_malformed_content_range() {
        let mut headers = HeaderMap::new();
        for raw in [
            "items 0-0/10",
            "bytes 2-1/10",
            "bytes 0-10/10",
            "bytes nonsense/10",
        ] {
            headers.insert(
                reqwest::header::CONTENT_RANGE,
                HeaderValue::from_bytes(raw.as_bytes()).unwrap(),
            );
            assert_eq!(content_range_total(&headers), None, "{raw}");
        }
    }
}
