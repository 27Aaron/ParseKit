//! Shared helpers used by multiple platform resolvers.
//!
//! Keep this module free of platform-specific endpoints, cookies, and CDN hosts.

use crate::{Error, Result};

/// Trailing punctuation often stuck to URLs when users paste share text.
pub const URL_TRAILING_PUNCT: &[char] = &[
    '。', '，', ',', '.', '！', '!', '？', '?', ')', '）', ']', '】', '、',
];

/// Strip chat-punctuation tails from a regex-matched URL candidate.
pub fn trim_url_candidate(matched: &str) -> &str {
    matched.trim_end_matches(URL_TRAILING_PUNCT)
}

/// Map a transport error without formatting `reqwest::Error` (may embed signed URLs).
pub fn map_network_error(error: &reqwest::Error, timeout_msg: &str, fail_msg: &str) -> Error {
    if error.is_timeout() {
        Error::Network(timeout_msg.into())
    } else {
        Error::Network(fail_msg.into())
    }
}

/// Default display title when a resolved post has no usable title field.
pub fn default_title_for_platform(platform: &str) -> &'static str {
    match platform {
        "wechat_channels" => "微信视频号视频",
        "douyin" => "抖音视频",
        _ => "视频",
    }
}

/// Platform-aware short title for delivery shells.
pub fn display_title_for_post(platform: &str, title: Option<&str>) -> String {
    title
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_title_for_platform(platform))
        .chars()
        .take(180)
        .collect()
}

/// Platform-aware short title from a resolved post.
pub fn display_title(post: &crate::ResolvedPost) -> String {
    display_title_for_post(post.platform.as_str(), post.title.as_deref())
}

/// Read at most `max_bytes` from a response body; reject oversized payloads.
pub async fn read_body_limited(
    response: reqwest::Response,
    max_bytes: usize,
    map_error: impl Fn(&reqwest::Error) -> Error,
) -> Result<Vec<u8>> {
    use futures_util::StreamExt;

    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(Error::UpstreamChanged);
    }

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| map_error(&error))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_common_chat_punctuation() {
        assert_eq!(
            trim_url_candidate("https://v.douyin.com/abc/。"),
            "https://v.douyin.com/abc/"
        );
        assert_eq!(
            trim_url_candidate("https://weixin.qq.com/sph/x】"),
            "https://weixin.qq.com/sph/x"
        );
    }

    #[test]
    fn display_title_prefers_non_empty_title() {
        assert_eq!(display_title_for_post("douyin", Some("  标题  ")), "标题");
        assert_eq!(
            display_title_for_post("wechat_channels", None),
            "微信视频号视频"
        );
        assert_eq!(display_title_for_post("unknown", Some("")), "视频");
    }
}
