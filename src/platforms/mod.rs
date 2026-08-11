//! Platform registration, resolver contracts, and implementations.

use std::future::Future;

use url::Url;

use crate::{Error, PlatformId, ResolvedPost, Result, media::DownloadRequestIdentity};

pub mod bilibili;
pub mod douyin;
pub mod util;
pub mod wechat;

pub use bilibili::BilibiliResolver;
pub use douyin::DouyinResolver;
pub use wechat::WechatResolver;

/// Static routing and download policy for one platform.
#[derive(Clone, Copy)]
pub struct PlatformSpec {
    id: PlatformId,
    capability_note: &'static str,
    extract_share_url: fn(&str) -> Result<Url>,
    reviewed_media_hosts: &'static [&'static str],
    download_identity: fn() -> DownloadRequestIdentity,
}

impl PlatformSpec {
    pub const fn new(
        id: PlatformId,
        capability_note: &'static str,
        extract_share_url: fn(&str) -> Result<Url>,
        reviewed_media_hosts: &'static [&'static str],
        download_identity: fn() -> DownloadRequestIdentity,
    ) -> Self {
        Self {
            id,
            capability_note,
            extract_share_url,
            reviewed_media_hosts,
            download_identity,
        }
    }

    pub const fn id(self) -> PlatformId {
        self.id
    }

    pub const fn capability_note(self) -> &'static str {
        self.capability_note
    }

    pub fn extract_share_url(self, input: &str) -> Result<Url> {
        (self.extract_share_url)(input)
    }

    pub const fn reviewed_media_hosts(self) -> &'static [&'static str] {
        self.reviewed_media_hosts
    }

    pub fn download_identity(self) -> DownloadRequestIdentity {
        (self.download_identity)()
    }
}

impl std::fmt::Debug for PlatformSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlatformSpec")
            .field("id", &self.id)
            .field("capability_note", &self.capability_note)
            .field("reviewed_media_hosts", &self.reviewed_media_hosts)
            .finish_non_exhaustive()
    }
}

/// Registration order used for share-text matching.
pub const PLATFORM_SPECS: &[&PlatformSpec] = &[&wechat::SPEC, &douyin::SPEC, &bilibili::SPEC];

/// Returns the registered spec for a stable platform id.
pub fn platform_spec(id: PlatformId) -> &'static PlatformSpec {
    match id {
        PlatformId::Wechat => &wechat::SPEC,
        PlatformId::Douyin => &douyin::SPEC,
        PlatformId::Bilibili => &bilibili::SPEC,
    }
}

/// Common interface for platform-specific URL resolution.
pub trait PlatformResolver: Sync {
    fn spec(&self) -> &'static PlatformSpec;

    fn platform_id(&self) -> PlatformId {
        self.spec().id()
    }

    /// Returns [`Error::UnsupportedUrl`] when the input belongs to another platform.
    fn extract_share_url(&self, input: &str) -> Result<Url> {
        self.spec().extract_share_url(input)
    }

    fn resolve_text(&self, input: &str) -> impl Future<Output = Result<ResolvedPost>> + Send {
        async move {
            let url = self.extract_share_url(input)?;
            self.resolve_url(&url).await
        }
    }

    fn resolve_url(&self, url: &Url) -> impl Future<Output = Result<ResolvedPost>> + Send;
}

#[derive(Debug, Clone)]
pub enum Platform {
    Wechat(WechatResolver),
    Douyin(DouyinResolver),
    Bilibili(BilibiliResolver),
}

impl Platform {
    pub fn spec(&self) -> &'static PlatformSpec {
        match self {
            Self::Wechat(resolver) => resolver.spec(),
            Self::Douyin(resolver) => resolver.spec(),
            Self::Bilibili(resolver) => resolver.spec(),
        }
    }

    pub fn platform_id(&self) -> PlatformId {
        self.spec().id()
    }

    /// Returns the display label.
    pub fn display_name(&self) -> &'static str {
        self.platform_id().display_name()
    }

    /// Describes resolver requirements.
    pub fn capability_note(&self) -> &'static str {
        self.spec().capability_note()
    }

    pub fn extract_share_url(&self, input: &str) -> Result<Url> {
        self.spec().extract_share_url(input)
    }

    pub async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        match self {
            Self::Wechat(resolver) => PlatformResolver::resolve_text(resolver, input).await,
            Self::Douyin(resolver) => PlatformResolver::resolve_text(resolver, input).await,
            Self::Bilibili(resolver) => PlatformResolver::resolve_text(resolver, input).await,
        }
    }

    pub async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        match self {
            Self::Wechat(resolver) => PlatformResolver::resolve_url(resolver, url).await,
            Self::Douyin(resolver) => PlatformResolver::resolve_url(resolver, url).await,
            Self::Bilibili(resolver) => PlatformResolver::resolve_url(resolver, url).await,
        }
    }

    pub fn as_wechat(&self) -> Option<&WechatResolver> {
        match self {
            Self::Wechat(resolver) => Some(resolver),
            _ => None,
        }
    }

    pub fn as_douyin(&self) -> Option<&DouyinResolver> {
        match self {
            Self::Douyin(resolver) => Some(resolver),
            _ => None,
        }
    }

    pub fn as_bilibili(&self) -> Option<&BilibiliResolver> {
        match self {
            Self::Bilibili(resolver) => Some(resolver),
            _ => None,
        }
    }
}

pub fn extract_share_url(input: &str) -> Result<Url> {
    for spec in PLATFORM_SPECS {
        match spec.extract_share_url(input) {
            Ok(url) => return Ok(url),
            Err(Error::UnsupportedUrl) => {}
            Err(error) => return Err(error),
        }
    }
    Err(Error::UnsupportedUrl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_share_url_accepts_wechat() {
        let url = extract_share_url("看看这个 https://weixin.qq.com/sph/AzJ7CGPYWD 不错")
            .expect("wechat share url should match");
        assert_eq!(url.as_str(), "https://weixin.qq.com/sph/AzJ7CGPYWD");
    }

    #[test]
    fn extract_share_url_accepts_douyin() {
        let url = extract_share_url("分享 https://v.douyin.com/q75E3VmAe6A/ 给你")
            .expect("douyin share url should match");
        assert_eq!(url.as_str(), "https://v.douyin.com/q75E3VmAe6A/");
    }

    #[test]
    fn extract_share_url_accepts_bilibili() {
        let url = extract_share_url("https://www.bilibili.com/video/BV1GJ411x7h7")
            .expect("bilibili url should match");
        assert!(url.as_str().contains("BV1GJ411x7h7"));
    }

    #[test]
    fn extract_share_url_rejects_unknown() {
        let err = extract_share_url("https://www.example.com/video/1")
            .expect_err("unknown host should be unsupported");
        assert!(matches!(err, Error::UnsupportedUrl));
    }

    #[test]
    fn registry_covers_every_platform_in_match_order() {
        assert_eq!(
            PLATFORM_SPECS
                .iter()
                .map(|spec| spec.id())
                .collect::<Vec<_>>(),
            PlatformId::ALL
        );
    }
}
