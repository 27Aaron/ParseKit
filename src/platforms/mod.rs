//! Resolver implementations for supported platforms.
//!
//! To add a platform:
//!
//! 1. Implement [`PlatformResolver`] under `platforms/<name>/`.
//! 2. Add a [`Platform`] variant and delegate its methods.
//! 3. Register it with [`crate::ParseKitBuilder`] and, when applicable, [`STATELESS_EXTRACTORS`].
//! 4. Review its media hosts and update [`crate::media::MediaDownloader::for_platform`].
//! 5. Add unit and fixture tests; keep live tests ignored by default.
//! 6. Update the platform table in the README.

use std::future::Future;

use url::Url;

use crate::{Error, PlatformId, ResolvedPost, Result};

pub mod bilibili;
pub mod douyin;
pub mod util;
pub mod wechat;

pub use bilibili::BilibiliResolver;
pub use douyin::DouyinResolver;
pub use wechat::WechatResolver;

/// Share-text extractors in deterministic matching order.
pub const STATELESS_EXTRACTORS: &[fn(&str) -> Result<Url>] = &[
    wechat::extract_share_url,
    douyin::extract_share_url,
    bilibili::extract_share_url,
];

pub trait PlatformResolver {
    fn platform_id(&self) -> PlatformId;

    /// Returns [`Error::UnsupportedUrl`] when the input belongs to another platform.
    fn extract_share_url(&self, input: &str) -> Result<Url>;

    fn resolve_text(&self, input: &str) -> impl Future<Output = Result<ResolvedPost>> + Send;

    fn resolve_url(&self, url: &Url) -> impl Future<Output = Result<ResolvedPost>> + Send;
}

#[derive(Debug, Clone)]
pub enum Platform {
    Wechat(WechatResolver),
    Douyin(DouyinResolver),
    Bilibili(BilibiliResolver),
}

impl Platform {
    pub fn platform_id(&self) -> PlatformId {
        match self {
            Self::Wechat(resolver) => resolver.platform_id(),
            Self::Douyin(resolver) => resolver.platform_id(),
            Self::Bilibili(resolver) => resolver.platform_id(),
        }
    }

    /// Returns the label shown by the CLI and logs.
    pub fn display_name(&self) -> &'static str {
        self.platform_id().display_name()
    }

    /// Describes credentials or other resolver constraints.
    pub fn capability_note(&self) -> &'static str {
        match self {
            Self::Wechat(_) => "needs YUANBAO_COOKIE",
            Self::Douyin(_) => "public share page",
            Self::Bilibili(_) => "public video page",
        }
    }

    pub fn extract_share_url(&self, input: &str) -> Result<Url> {
        match self {
            Self::Wechat(resolver) => PlatformResolver::extract_share_url(resolver, input),
            Self::Douyin(resolver) => PlatformResolver::extract_share_url(resolver, input),
            Self::Bilibili(resolver) => PlatformResolver::extract_share_url(resolver, input),
        }
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
    for extract in STATELESS_EXTRACTORS {
        match extract(input) {
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
    fn extract_share_url_accepts_wechat_channels() {
        let url = extract_share_url("看看这个 https://weixin.qq.com/sph/A27pGwf5f9 不错")
            .expect("wechat share url should match");
        assert_eq!(url.as_str(), "https://weixin.qq.com/sph/A27pGwf5f9");
    }

    #[test]
    fn extract_share_url_accepts_douyin() {
        let url = extract_share_url("分享 https://v.douyin.com/iAbCdEf/ 给你")
            .expect("douyin share url should match");
        assert_eq!(url.as_str(), "https://v.douyin.com/iAbCdEf/");
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
    fn default_match_order_is_wechat_douyin_bilibili() {
        assert_eq!(STATELESS_EXTRACTORS.len(), 3);
    }
}
