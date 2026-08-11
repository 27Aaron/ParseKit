//! Share URL extraction and Bilibili video identifier parsing.

use regex::Regex;
use url::Url;

use crate::{
    Error, Result,
    platforms::util::trim_url_candidate,
    url::{CleanPolicy, clean_tracking_params},
};

#[derive(Debug, Clone)]
pub(super) enum VideoId {
    Bvid(String),
    Aid(u64),
}

pub fn extract_share_url(input: &str) -> Result<Url> {
    static BV_PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static AV_PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static B23_PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();

    let bv = BV_PATTERN.get_or_init(|| {
        Regex::new(r#"https?://(?:(?:www|m)\.)?bilibili\.com/video/(BV[0-9A-Za-z]+)"#)
            .expect("constant Bilibili BV regex must compile")
    });
    let av = AV_PATTERN.get_or_init(|| {
        Regex::new(r#"(?i)https?://(?:(?:www|m)\.)?bilibili\.com/video/av(\d+)"#)
            .expect("constant Bilibili av regex must compile")
    });
    let b23 = B23_PATTERN.get_or_init(|| {
        Regex::new(r#"https?://b23\.tv/[A-Za-z0-9]+"#)
            .expect("constant Bilibili b23 regex must compile")
    });

    for pattern in [bv, av] {
        for matched in pattern.find_iter(input) {
            let candidate = trim_url_candidate(matched.as_str());
            let Ok(mut url) = Url::parse(candidate) else {
                continue;
            };
            normalize_https(&mut url);
            if parse_video_id(&url).is_ok() {
                return Ok(clean_tracking_params(&url, CleanPolicy::SHARE_PAGE));
            }
        }
    }
    for matched in b23.find_iter(input) {
        let candidate = trim_url_candidate(matched.as_str());
        if let Ok(mut url) = Url::parse(candidate) {
            normalize_https(&mut url);
            return Ok(url);
        }
    }
    Err(Error::UnsupportedUrl)
}

fn normalize_https(url: &mut Url) {
    if url.scheme() == "http" {
        let _ = url.set_scheme("https");
    }
}

pub(super) fn parse_video_id(url: &Url) -> Result<VideoId> {
    if url.scheme() != "https" {
        return Err(Error::UnsupportedUrl);
    }
    let host = url
        .host_str()
        .map(|host| host.to_ascii_lowercase())
        .ok_or(Error::UnsupportedUrl)?;
    if host == "b23.tv" {
        return Err(Error::UnsupportedUrl);
    }
    if !matches!(
        host.as_str(),
        "www.bilibili.com" | "bilibili.com" | "m.bilibili.com"
    ) {
        return Err(Error::UnsupportedUrl);
    }
    let Some(id) = url
        .path()
        .strip_prefix("/video/")
        .and_then(|rest| rest.split('/').next())
    else {
        return Err(Error::UnsupportedUrl);
    };
    if id.starts_with("BV") && id.len() >= 6 {
        return Ok(VideoId::Bvid(id.to_owned()));
    }
    if let Some(aid) = id
        .strip_prefix("av")
        .or_else(|| id.strip_prefix("AV"))
        .and_then(|value| value.parse().ok())
    {
        return Ok(VideoId::Aid(aid));
    }
    Err(Error::UnsupportedUrl)
}

pub(super) fn is_b23_host(url: &Url) -> bool {
    url.host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("b23.tv"))
}
