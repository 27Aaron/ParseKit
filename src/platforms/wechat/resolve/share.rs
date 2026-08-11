//! Share URL extraction and media URL derivation.

use regex::Regex;
use url::Url;

use crate::{
    Error, Result, media::host::is_reviewed_https_url, platforms::util::trim_url_candidate,
    platforms::wechat::hosts::REVIEWED_MEDIA_HOSTS,
};

pub(super) struct NormalizedShareUrl {
    pub(super) share_id: String,
    pub(super) canonical_url: Url,
}

pub fn extract_share_url(input: &str) -> Result<Url> {
    static URL_PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let pattern = URL_PATTERN.get_or_init(|| {
        Regex::new(r#"https://weixin\.qq\.com/sph/[^\s<>\"']+"#)
            .expect("constant URL regex must compile")
    });

    for matched in pattern.find_iter(input) {
        let candidate = trim_url_candidate(matched.as_str());
        let Ok(url) = Url::parse(candidate) else {
            continue;
        };
        if let Ok(normalized) = normalize_share_url(&url) {
            return Ok(normalized.canonical_url);
        }
    }
    Err(Error::UnsupportedUrl)
}

pub(super) fn normalize_share_url(url: &Url) -> Result<NormalizedShareUrl> {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::UnsupportedUrl);
    }

    let host = url.host_str().ok_or(Error::UnsupportedUrl)?;
    if host.ends_with('.') {
        return Err(Error::UnsupportedUrl);
    }
    let host = host.to_ascii_lowercase();

    if host != "weixin.qq.com" {
        return Err(Error::UnsupportedUrl);
    }
    let share_id = url
        .path()
        .strip_prefix("/sph/")
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .ok_or(Error::UnsupportedUrl)?
        .to_owned();

    if !(6..=128).contains(&share_id.len())
        || !share_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(Error::UnsupportedUrl);
    }

    let canonical_url = Url::parse(&format!("https://weixin.qq.com/sph/{share_id}"))
        .expect("validated share id always creates a URL");
    Ok(NormalizedShareUrl {
        share_id,
        canonical_url,
    })
}

pub fn derive_direct_media_url(source: &Url) -> Option<Url> {
    if !is_allowed_media_url(source) {
        return None;
    }

    let file_key = query_value(source, "encfilekey")?;
    let token = query_value(source, "token")?;
    let mut direct = source.clone();
    direct.set_query(None);
    direct.set_fragment(None);
    direct
        .query_pairs_mut()
        .append_pair("encfilekey", &file_key)
        .append_pair("token", &token);

    Some(direct)
}

pub(super) fn is_allowed_media_url(url: &Url) -> bool {
    is_reviewed_https_url(url, REVIEWED_MEDIA_HOSTS)
}

pub(super) fn endpoint_is_loopback_http(url: &Url) -> bool {
    url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
}

pub(super) fn query_value(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find_map(|(key, value)| (key == name && !value.is_empty()).then(|| value.into_owned()))
}
