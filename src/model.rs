use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    H264,
    H265,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaSourceKind {
    H264,
    H265,
    Generic,
    Direct,
    Derived,
}

#[derive(Clone)]
pub struct MediaSource {
    pub url: Url,
    pub codec: VideoCodec,
    pub provenance: MediaSourceKind,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_hint: Option<u64>,
    pub decode_key: Option<u64>,
}

impl fmt::Debug for MediaSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MediaSource")
            .field("host", &self.url.host_str().unwrap_or("<invalid>"))
            .field("url", &"<redacted>")
            .field("codec", &self.codec)
            .field("provenance", &self.provenance)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("size_hint", &self.size_hint)
            .field("has_decode_key", &self.decode_key.is_some())
            .finish()
    }
}

/// Platform-agnostic resolved post ready for download / delivery.
#[derive(Clone)]
pub struct ResolvedPost {
    pub platform: String,
    pub post_id: String,
    pub canonical_url: Url,
    pub title: Option<String>,
    pub cover_url: Option<Url>,
    pub video: MediaSource,
    pub fallback_videos: Vec<MediaSource>,
}

impl fmt::Debug for ResolvedPost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedPost")
            .field("platform", &self.platform)
            .field("post_id", &self.post_id)
            .field("canonical_url", &"<redacted>")
            .field("title", &self.title)
            .field("has_cover", &self.cover_url.is_some())
            .field("video", &self.video)
            .field("fallback_video_count", &self.fallback_videos.len())
            .finish()
    }
}

impl ResolvedPost {
    pub fn media_sources(&self) -> impl Iterator<Item = &MediaSource> {
        std::iter::once(&self.video).chain(self.fallback_videos.iter())
    }

    /// Short title for display using a platform-agnostic fallback (`"视频"`).
    ///
    /// For platform-specific defaults (e.g. `"微信视频号视频"`), use
    /// [`crate::platforms::util::display_title_for_post`].
    pub fn display_title(&self) -> String {
        self.display_title_or("视频")
    }

    /// Short title for display with a caller-supplied fallback.
    pub fn display_title_or(&self, fallback: &str) -> String {
        self.title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback)
            .chars()
            .take(180)
            .collect()
    }
}
