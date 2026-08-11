//! Host allowlist, DNS resolve, and private-IP rejection (SSRF guard).

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
    if url.port().is_some_and(|port| port != 443) {
        return Err(Error::Download("媒体地址不能使用非标准端口".to_owned()));
    }
    if url.fragment().is_some() {
        return Err(Error::Download("媒体地址不能包含片段标识".to_owned()));
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

pub(super) async fn resolve_public_addresses(host: &str) -> Result<Vec<SocketAddr>> {
    let addresses = timeout(DNS_TIMEOUT, tokio::net::lookup_host((host, 443)))
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
        || (a == 100 && (64..=127).contains(&b)) // carrier-grade NAT
        || (a == 169 && b == 254) // link-local and metadata endpoints
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2) // documentation
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || (a == 198 && b == 51 && c == 100) // documentation
        || (a == 203 && b == 0 && c == 113) // documentation
        || a >= 224 // multicast, reserved, and broadcast
}

pub(super) fn is_forbidden_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_forbidden_ipv4(mapped);
    }

    let segments = ip.segments();
    ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00 // unique-local fc00::/7
        || (segments[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
        || (segments[0] & 0xffc0) == 0xfec0 // deprecated site-local fec0::/10
        || (segments[0] == 0x0064 && segments[1] == 0xff9b) // NAT64 transition ranges
        || segments[0] == 0x2002 // 6to4
        || (segments[0] == 0x2001 && segments[1] == 0) // Teredo
        || (segments[0] == 0x2001 && segments[1] == 0x0db8) // documentation
}

pub(super) fn valid_host_name(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !host.contains("..")
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
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
