//! Shared helpers for platform resolvers.

use crate::{Error, PlatformId, Result, url};

pub use crate::url::{URL_TRAILING_PUNCT, trim_url_candidate};

/// Do not format `reqwest::Error` (may contain signed URLs).
pub fn map_network_error(error: &reqwest::Error, timeout_msg: &str, fail_msg: &str) -> Error {
    if error.is_timeout() {
        Error::Network(timeout_msg.into())
    } else {
        Error::Network(fail_msg.into())
    }
}

pub fn default_title_for_platform(platform: PlatformId) -> &'static str {
    match platform {
        PlatformId::WechatChannels => "微信视频号视频",
        PlatformId::Douyin => "抖音视频",
        PlatformId::Bilibili => "哔哩哔哩视频",
    }
}

pub fn display_title_for_post(platform: PlatformId, title: Option<&str>) -> String {
    title
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_title_for_platform(platform))
        .chars()
        .take(180)
        .collect()
}

pub fn display_title(post: &crate::ResolvedPost) -> String {
    display_title_for_post(post.platform, post.title.as_deref())
}

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

/// Re-export for callers that only depend on platforms::util historically.
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
