//! SSRF protection through host allowlists, DNS pinning, and public-IP checks.

use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use tokio::time::timeout;
use url::{Host, Url};

use crate::{Error, Result};

use crate::media::host::host_matches_rules;

pub(super) const DNS_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn validate_media_url<'a>(
    url: &'a Url,
    allowed_hosts: &HashSet<String>,
) -> Result<&'a str> {
    if url.scheme() != "https" {
        return Err(Error::Download("媒体地址必须使用 HTTPS".to_owned()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::Download("媒体地址不能包含用户凭据".to_owned()));
    }
    // CDN edges (e.g. Douyin jspcdn) may use non-443 HTTPS ports; host allowlist +
    // public-IP checks still apply. Reject only empty/invalid ports implicitly via URL.
    if url.fragment().is_some() {
        return Err(Error::Download("媒体地址不能包含片段标识".to_owned()));
    }
    if url.port() == Some(0) {
        return Err(Error::Download("媒体地址端口无效".to_owned()));
    }

    let host = match url.host() {
        Some(Host::Domain(host)) if !host.ends_with('.') => host,
        _ => return Err(Error::Download("媒体地址主机无效".to_owned())),
    };
    let normalized = host.to_ascii_lowercase();
    if !host_is_allowed(&normalized, allowed_hosts) {
        return Err(Error::Download("媒体地址主机不在允许列表中".to_owned()));
    }

    Ok(host)
}

pub(super) fn host_is_allowed(host: &str, allowed_hosts: &HashSet<String>) -> bool {
    host_matches_rules(host, allowed_hosts.iter().map(String::as_str))
}

/// Resolves `host` for connecting on `port` (must match the request URL port for pinning).
pub(super) async fn resolve_public_addresses(host: &str, port: u16) -> Result<Vec<SocketAddr>> {
    let addresses = timeout(DNS_TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| Error::Network("媒体主机 DNS 解析超时".to_owned()))?
        .map_err(|_| Error::Network("媒体主机 DNS 解析失败".to_owned()))?
        .collect::<Vec<_>>();

    if addresses.is_empty() {
        return Err(Error::Network("媒体主机没有可用 DNS 地址".to_owned()));
    }
    if addresses
        .iter()
        .any(|address| is_forbidden_ip(address.ip()))
    {
        return Err(Error::Download(
            "媒体主机解析到了不允许的网络地址".to_owned(),
        ));
    }

    Ok(addresses)
}

pub(super) fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_forbidden_ipv4(ip),
        IpAddr::V6(ip) => is_forbidden_ipv6(ip),
    }
}

pub(super) fn is_forbidden_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _d] = ip.octets();

    a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b)) // Shared address space (RFC 6598).
        || (a == 169 && b == 254) // Link-local; includes cloud metadata endpoints.
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2) // Documentation (TEST-NET-1).
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (b == 18 || b == 19)) // Benchmarking.
        || (a == 198 && b == 51 && c == 100) // Documentation (TEST-NET-2).
        || (a == 203 && b == 0 && c == 113) // Documentation (TEST-NET-3).
        || a >= 224 // Multicast, reserved, or limited broadcast.
}

pub(super) fn is_forbidden_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_forbidden_ipv4(mapped);
    }

    let segments = ip.segments();
    ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || segments[..6] == [0; 6] // Deprecated IPv4-compatible address space (::/96).
        || (segments[0] & 0xfe00) == 0xfc00 // Unique-local addresses.
        || (segments[0] & 0xffc0) == 0xfe80 // Link-local addresses.
        || (segments[0] & 0xffc0) == 0xfec0 // Deprecated site-local addresses.
        || (segments[0] == 0x0064
            && segments[1] == 0xff9b
            && (segments[2..6] == [0; 4] || segments[2] == 1)) // NAT64 prefixes.
        || (segments[0] == 0x0100 && segments[1] == 0 && segments[2] == 0 && segments[3] == 0) // Discard-only.
        || segments[0] == 0x2002 // 6to4.
        || (segments[0] == 0x2001 && segments[1] == 0) // Teredo.
        || (segments[0] == 0x2001 && segments[1] == 2) // Benchmarking.
        || (segments[0] == 0x2001 && (segments[1] & 0xfff0) == 0x0010) // ORCHID.
        || (segments[0] == 0x2001 && (segments[1] & 0xfff0) == 0x0020) // ORCHIDv2.
        || (segments[0] == 0x2001 && segments[1] == 0x0db8) // Documentation.
        || (segments[0] == 0x3fff && (segments[1] & 0xf000) == 0) // Documentation (3fff::/20).
        || segments[0] == 0x5f00 // Segment-routing SIDs.
}

pub(super) fn valid_host_name(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

pub(super) fn valid_allowlist_entry(entry: &str) -> bool {
    if let Some(suffix) = entry.strip_prefix('.') {
        valid_host_name(suffix) && suffix.contains('.')
    } else {
        valid_host_name(entry)
    }
}

pub(super) fn normalize_allowed_hosts(
    hosts: impl IntoIterator<Item = impl AsRef<str>>,
) -> Result<HashSet<String>> {
    let allowed_hosts = hosts
        .into_iter()
        .map(|host| host.as_ref().trim().to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .collect::<HashSet<_>>();
    if allowed_hosts.is_empty()
        || allowed_hosts
            .iter()
            .any(|host| !valid_allowlist_entry(host))
    {
        return Err(Error::Config("媒体主机允许列表无效".to_owned()));
    }
    Ok(allowed_hosts)
}
