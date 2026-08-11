use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::{Error, Result};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Video,
    ImageSet,
    Audio,
    Mixed,
    Unknown,
}

#[derive(Clone)]
pub enum MediaItem {
    Video {
        primary: MediaSource,
        fallbacks: Vec<MediaSource>,
    },
    Image {
        source: MediaSource,
    },
    Audio {
        source: MediaSource,
    },
}

impl fmt::Debug for MediaItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Video { primary, fallbacks } => formatter
                .debug_struct("Video")
                .field("primary", primary)
                .field("fallback_count", &fallbacks.len())
                .finish(),
            Self::Image { source } => formatter
                .debug_struct("Image")
                .field("source", source)
                .finish(),
            Self::Audio { source } => formatter
                .debug_struct("Audio")
                .field("source", source)
                .finish(),
        }
    }
}

impl MediaItem {
    pub fn as_video(&self) -> Option<(&MediaSource, &[MediaSource])> {
        match self {
            Self::Video { primary, fallbacks } => Some((primary, fallbacks.as_slice())),
            _ => None,
        }
    }

    pub fn sources(&self) -> Vec<&MediaSource> {
        match self {
            Self::Video { primary, fallbacks } => {
                let mut list = Vec::with_capacity(1 + fallbacks.len());
                list.push(primary);
                list.extend(fallbacks.iter());
                list
            }
            Self::Image { source } | Self::Audio { source } => vec![source],
        }
    }
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

/// Resolved post. Prefer [`Self::new_video`] so `kind` and `media` stay consistent.
#[derive(Clone)]
pub struct ResolvedPost {
    pub platform: String,
    pub post_id: String,
    pub canonical_url: Url,
    pub title: Option<String>,
    pub cover_url: Option<Url>,
    pub kind: ContentKind,
    pub media: Vec<MediaItem>,
}

impl fmt::Debug for ResolvedPost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedPost")
            .field("platform", &self.platform)
            .field("post_id", &self.post_id)
            .field("canonical_url", &"<redacted>")
            .field("title", &self.title)
            .field("kind", &self.kind)
            .field("media_count", &self.media.len())
            .field("has_cover", &self.cover_url.is_some())
            .field("primary_video", &self.primary_video())
            .finish()
    }
}

impl ResolvedPost {
    pub fn new_video(
        platform: impl Into<String>,
        post_id: impl Into<String>,
        canonical_url: Url,
        title: Option<String>,
        cover_url: Option<Url>,
        video: MediaSource,
        fallback_videos: Vec<MediaSource>,
    ) -> Self {
        Self {
            platform: platform.into(),
            post_id: post_id.into(),
            canonical_url,
            title,
            cover_url,
            kind: ContentKind::Video,
            media: vec![MediaItem::Video {
                primary: video,
                fallbacks: fallback_videos,
            }],
        }
    }

    /// All media sources in play order (primary video first, then fallbacks / other items).
    pub fn media_sources(&self) -> impl Iterator<Item = &MediaSource> {
        self.media.iter().flat_map(MediaItem::sources)
    }

    pub fn primary_video(&self) -> Option<&MediaSource> {
        self.media
            .iter()
            .find_map(|item| item.as_video().map(|(primary, _)| primary))
    }

    /// Fallback video sources for the first video item (excludes primary).
    pub fn video_fallbacks(&self) -> &[MediaSource] {
        self.media
            .iter()
            .find_map(|item| item.as_video().map(|(_, fallbacks)| fallbacks))
            .unwrap_or(&[])
    }

    pub fn require_primary_video(&self) -> Result<&MediaSource> {
        self.primary_video().ok_or(Error::MediaUnavailable)
    }

    pub fn display_title(&self) -> String {
        self.display_title_or("视频")
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_source(path: &str) -> MediaSource {
        MediaSource {
            url: Url::parse(&format!("https://cdn.example/{path}")).unwrap(),
            codec: VideoCodec::H264,
            provenance: MediaSourceKind::Direct,
            width: Some(720),
            height: Some(1280),
            size_hint: None,
            decode_key: None,
        }
    }

    #[test]
    fn new_video_stores_primary_and_fallbacks_in_media() {
        let primary = sample_source("a.mp4");
        let fallback = sample_source("b.mp4");
        let post = ResolvedPost::new_video(
            "douyin",
            "1",
            Url::parse("https://www.douyin.com/video/1").unwrap(),
            Some("t".into()),
            None,
            primary.clone(),
            vec![fallback.clone()],
        );
        assert_eq!(post.kind, ContentKind::Video);
        assert_eq!(post.media.len(), 1);
        assert_eq!(
            post.primary_video().map(|s| s.url.as_str()),
            Some(primary.url.as_str())
        );
        assert_eq!(post.video_fallbacks().len(), 1);
        assert_eq!(post.video_fallbacks()[0].url, fallback.url);
        assert_eq!(post.media_sources().count(), 2);
        assert_eq!(
            post.require_primary_video().unwrap().url.as_str(),
            primary.url.as_str()
        );
    }
}
