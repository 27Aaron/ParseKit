use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::{Error, Result};

pub(crate) fn format_title(title: Option<&str>, fallback: &str) -> String {
    title
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .chars()
        .take(180)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlatformId {
    #[serde(rename = "wechat")]
    Wechat,
    #[serde(rename = "douyin")]
    Douyin,
    #[serde(rename = "bilibili")]
    Bilibili,
}

impl PlatformId {
    pub const ALL: [Self; 3] = [Self::Wechat, Self::Douyin, Self::Bilibili];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wechat => "wechat",
            Self::Douyin => "douyin",
            Self::Bilibili => "bilibili",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Wechat => "微信视频号",
            Self::Douyin => "抖音",
            Self::Bilibili => "哔哩哔哩",
        }
    }

    pub const fn default_title(self) -> &'static str {
        match self {
            Self::Wechat => "微信视频号视频",
            Self::Douyin => "抖音视频",
            Self::Bilibili => "哔哩哔哩视频",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "wechat" => Some(Self::Wechat),
            "douyin" => Some(Self::Douyin),
            "bilibili" => Some(Self::Bilibili),
            _ => None,
        }
    }
}

impl fmt::Display for PlatformId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

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
        self.iter_sources().collect()
    }

    fn iter_sources(&self) -> impl Iterator<Item = &MediaSource> {
        let (primary, fallbacks) = match self {
            Self::Video { primary, fallbacks } => (primary, fallbacks.as_slice()),
            Self::Image { source } | Self::Audio { source } => (source, &[][..]),
        };
        std::iter::once(primary).chain(fallbacks)
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

/// A normalized post returned by a platform resolver.
///
/// Use [`Self::new_video`] or [`Self::new_image_set`] to keep `kind` and `media` consistent.
#[derive(Clone)]
pub struct ResolvedPost {
    pub platform: PlatformId,
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
        platform: PlatformId,
        post_id: impl Into<String>,
        canonical_url: Url,
        title: Option<String>,
        cover_url: Option<Url>,
        video: MediaSource,
        fallback_videos: Vec<MediaSource>,
    ) -> Self {
        Self {
            platform,
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

    pub fn new_image_set(
        platform: PlatformId,
        post_id: impl Into<String>,
        canonical_url: Url,
        title: Option<String>,
        cover_url: Option<Url>,
        images: Vec<MediaSource>,
    ) -> Result<Self> {
        if images.is_empty() {
            return Err(Error::MediaUnavailable);
        }
        Ok(Self {
            platform,
            post_id: post_id.into(),
            canonical_url,
            title,
            cover_url,
            kind: ContentKind::ImageSet,
            media: images
                .into_iter()
                .map(|source| MediaItem::Image { source })
                .collect(),
        })
    }

    /// Iterates sources in playback order: primary, fallbacks, then other items.
    pub fn media_sources(&self) -> impl Iterator<Item = &MediaSource> {
        self.media.iter().flat_map(MediaItem::iter_sources)
    }

    pub fn primary_video(&self) -> Option<&MediaSource> {
        self.media
            .iter()
            .find_map(|item| item.as_video().map(|(primary, _)| primary))
    }

    /// Returns fallback sources for the first video item, excluding its primary source.
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
        self.display_title_or(self.platform.default_title())
    }

    pub fn display_title_or(&self, fallback: &str) -> String {
        format_title(self.title.as_deref(), fallback)
    }

    /// `{platform}_{canonical_path_slug}` without extension.
    pub fn download_file_stem(&self) -> String {
        download_file_stem(self.platform, &self.canonical_url, &self.post_id)
    }
}

pub fn download_file_stem(platform: PlatformId, canonical_url: &Url, post_id: &str) -> String {
    let from_canonical = canonical_url
        .path_segments()
        .into_iter()
        .flatten()
        .rfind(|segment| !segment.is_empty());
    let slug = sanitize_filename_component(from_canonical.unwrap_or(post_id));
    let slug = if slug.is_empty() {
        sanitize_filename_component(post_id)
    } else {
        slug
    };
    let slug = if slug.is_empty() {
        "media".to_owned()
    } else {
        slug
    };
    format!("{}_{slug}", platform.as_str())
}

pub(crate) fn sanitize_filename_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(120));
    for ch in raw.chars().take(120) {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => out.push(ch),
            _ => {
                if !out.ends_with('_') {
                    out.push('_');
                }
            }
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    while out.starts_with('_') {
        out.remove(0);
    }
    out
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
    fn download_file_stem_uses_platform_and_canonical_path_slug() {
        let post = ResolvedPost::new_video(
            PlatformId::Wechat,
            "export/UzFfBgAAxN6nSDBBUACfjMzT4DCgIrzfaAMsA2Z5MZdjdIWLCi-NdGBi-Q",
            Url::parse("https://weixin.qq.com/sph/AzJ7CGPYWD").unwrap(),
            Some("免费领强力道具！零氪也能当大佬！".into()),
            None,
            sample_source("v.mp4"),
            Vec::new(),
        );
        assert_eq!(post.download_file_stem(), "wechat_AzJ7CGPYWD");

        assert_eq!(
            download_file_stem(
                PlatformId::Douyin,
                &Url::parse("https://www.douyin.com/video/7661946724177829115").unwrap(),
                "7661946724177829115",
            ),
            "douyin_7661946724177829115"
        );
        assert_eq!(
            download_file_stem(
                PlatformId::Bilibili,
                &Url::parse("https://www.bilibili.com/video/BV1GJ411x7h7").unwrap(),
                "170001",
            ),
            "bilibili_BV1GJ411x7h7"
        );
    }

    #[test]
    fn sanitize_filename_component_strips_path_separators() {
        assert_eq!(
            sanitize_filename_component("export/UzFf-BgAA"),
            "export_UzFf-BgAA"
        );
        assert_eq!(sanitize_filename_component("___"), "");
    }

    #[test]
    fn new_video_stores_primary_and_fallbacks_in_media() {
        let primary = sample_source("a.mp4");
        let fallback = sample_source("b.mp4");
        let post = ResolvedPost::new_video(
            PlatformId::Douyin,
            "7661946724177829115",
            Url::parse("https://www.douyin.com/video/7661946724177829115").unwrap(),
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

    #[test]
    fn image_sets_require_at_least_one_source() {
        let result = ResolvedPost::new_image_set(
            PlatformId::Douyin,
            "7661946724177829115",
            Url::parse("https://www.douyin.com/note/7661946724177829115").unwrap(),
            None,
            None,
            Vec::new(),
        );
        assert!(matches!(result, Err(Error::MediaUnavailable)));
    }

    #[test]
    fn display_title_uses_platform_default_and_unicode_safe_limit() {
        let mut post = ResolvedPost::new_video(
            PlatformId::Bilibili,
            "BV1GJ411x7h7",
            Url::parse("https://www.bilibili.com/video/BV1GJ411x7h7").unwrap(),
            Some("  ".into()),
            None,
            sample_source("a.mp4"),
            Vec::new(),
        );
        assert_eq!(post.display_title(), "哔哩哔哩视频");

        post.title = Some("界".repeat(200));
        assert_eq!(post.display_title().chars().count(), 180);
    }
}
