//! Extract Douyin share URLs and aweme identifiers.

use regex::Regex;
use url::Url;

use crate::{Error, Result, platforms::util::trim_url_candidate};

const REDIRECT_HOSTS: &[&str] = &[
    "v.douyin.com",
    "www.douyin.com",
    "m.douyin.com",
    "www.iesdouyin.com",
    "iesdouyin.com",
];

pub fn extract_share_url(input: &str) -> Result<Url> {
    static URL_PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let pattern = URL_PATTERN.get_or_init(|| {
        Regex::new(
            r#"(?i)https?://(?:(?:v|www|m)\.)?douyin\.com/[^\s<>"']+|https?://(?:www\.)?iesdouyin\.com/[^\s<>"']+"#,
        )
        .expect("constant Douyin URL regex must compile")
    });

    for matched in pattern.find_iter(input) {
        let candidate = trim_url_candidate(matched.as_str());
        let Ok(mut url) = Url::parse(candidate) else {
            continue;
        };
        if url.scheme() != "http" && url.scheme() != "https" {
            continue;
        }
        if url.scheme() == "http" {
            let _ = url.set_scheme("https");
        }
        if is_douyin_host(&url) && !is_excluded_path(&url) {
            return Ok(url);
        }
    }
    Err(Error::UnsupportedUrl)
}

fn is_douyin_host(url: &Url) -> bool {
    let Some(host) = url.host_str().map(|host| host.to_ascii_lowercase()) else {
        return false;
    };
    matches!(
        host.as_str(),
        "v.douyin.com"
            | "www.douyin.com"
            | "m.douyin.com"
            | "douyin.com"
            | "www.iesdouyin.com"
            | "iesdouyin.com"
    )
}

pub(super) fn is_short_link_host(url: &Url) -> bool {
    url.host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("v.douyin.com"))
}

pub(super) fn is_allowed_redirect_host(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|host| {
            REDIRECT_HOSTS
                .iter()
                .any(|allowed| host.eq_ignore_ascii_case(allowed))
        })
}

fn is_excluded_path(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    path.starts_with("/share/user") || path.starts_with("/qishui") || path.starts_with("/user/")
}

pub(super) fn extract_aweme_id(input: &str) -> Option<String> {
    static PATTERNS: std::sync::OnceLock<Vec<Regex>> = std::sync::OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            r"(?i)/(?:share/)?video/(\d{5,32})",
            r"(?i)/note/(\d{5,32})",
            r"(?i)[?&]modal_id=(\d{5,32})",
            r"(?i)[?&]vid=(\d{5,32})",
        ]
        .into_iter()
        .map(|pattern| Regex::new(pattern).expect("constant aweme id regex must compile"))
        .collect()
    });

    patterns.iter().find_map(|pattern| {
        pattern
            .captures(input)
            .and_then(|capture| capture.get(1))
            .map(|id| id.as_str().to_owned())
    })
}
