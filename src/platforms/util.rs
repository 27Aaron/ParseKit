//! Common resolver utilities.

use crate::{Error, PlatformId, Result, url};

pub use crate::url::{URL_TRAILING_PUNCT, trim_url_candidate};

/// Maps a request failure without formatting it, which could expose signed URLs.
pub fn map_network_error(error: &reqwest::Error, timeout_msg: &str, fail_msg: &str) -> Error {
    if error.is_timeout() {
        Error::Network(timeout_msg.into())
    } else {
        Error::Network(fail_msg.into())
    }
}

pub fn default_title_for_platform(platform: PlatformId) -> &'static str {
    platform.default_title()
}

pub fn display_title_for_post(platform: PlatformId, title: Option<&str>) -> String {
    crate::model::format_title(title, default_title_for_platform(platform))
}

pub fn display_title(post: &crate::ResolvedPost) -> String {
    post.display_title()
}

pub async fn read_body_limited(
    mut response: reqwest::Response,
    max_bytes: usize,
    map_error: impl Fn(&reqwest::Error) -> Error,
) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(Error::UpstreamChanged);
    }

    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| map_error(&error))? {
        let next_len = bytes
            .len()
            .checked_add(chunk.len())
            .filter(|length| *length <= max_bytes)
            .ok_or(Error::UpstreamChanged)?;
        bytes.reserve(next_len.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Retained for compatibility with callers of `platforms::util`.
pub use url::clean_tracking_params;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_title_prefers_non_empty_title() {
        assert_eq!(
            display_title_for_post(PlatformId::Douyin, Some("  标题  ")),
            "标题"
        );
        assert_eq!(
            display_title_for_post(PlatformId::WechatChannels, None),
            "微信视频号视频"
        );
        assert_eq!(
            display_title_for_post(PlatformId::Bilibili, Some("")),
            "哔哩哔哩视频"
        );
    }
}
