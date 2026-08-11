//! Multi-platform resolvers and registration.
//!
//! # Adding a platform
//!
//! 1. Add `platforms/<name>.rs` (or a subdirectory) with a concrete resolver.
//! 2. Implement [`PlatformResolver`] for that type.
//! 3. Add a variant to [`Platform`] and forward methods in its `impl`.
//! 4. Register it via [`crate::ParseKit::builder`] (order is match order).
//! 5. If the matcher is stateless, also list its free `extract_share_url` in
//!    [`STATELESS_EXTRACTORS`] so [`crate::ParseKit::extract_share_url`] works
//!    without a hub instance.
//!
//! Platform-private data (CDN hosts, cookies, decrypt) stays under that
//! platform's module — not in shared `model` / `media`.

use std::future::Future;

use url::Url;

use crate::{Error, ResolvedPost, Result};

pub mod douyin;
pub mod util;
pub mod wechat;

pub use douyin::DouyinResolver;
pub use wechat::WechatResolver;

/// Stateless URL matchers in default registration order (WeChat, then Douyin).
///
/// This is the single source of truth for match order used by
/// [`extract_share_url`] and documented for [`crate::ParseKit::builder`].
pub const STATELESS_EXTRACTORS: &[fn(&str) -> Result<Url>] =
    &[wechat::extract_share_url, douyin::extract_share_url];

/// Contract every platform resolver implements.
///
/// Implement this on a concrete type, then register it via [`Platform`] so
/// [`crate::ParseKit`] can dispatch without product code changes.
pub trait PlatformResolver {
    /// Stable id written into [`ResolvedPost::platform`].
    fn platform_id(&self) -> &'static str;

    /// Extract a canonical share URL for this platform from free-form text.
    ///
    /// Return [`Error::UnsupportedUrl`] when the input is not for this platform.
    fn extract_share_url(&self, input: &str) -> Result<Url>;

    /// Resolve free-form share text into a post.
    fn resolve_text(&self, input: &str) -> impl Future<Output = Result<ResolvedPost>> + Send;

    /// Resolve an already-parsed URL into a post.
    fn resolve_url(&self, url: &Url) -> impl Future<Output = Result<ResolvedPost>> + Send;
}

/// Registered platform backends for this build.
///
/// New platforms are added as enum variants. The hub stores them in a `Vec`
/// and tries them in order.
#[derive(Debug, Clone)]
pub enum Platform {
    Wechat(WechatResolver),
    Douyin(DouyinResolver),
}

impl Platform {
    pub fn platform_id(&self) -> &'static str {
        match self {
            Self::Wechat(resolver) => resolver.platform_id(),
            Self::Douyin(resolver) => resolver.platform_id(),
        }
    }

    pub fn extract_share_url(&self, input: &str) -> Result<Url> {
        match self {
            Self::Wechat(resolver) => PlatformResolver::extract_share_url(resolver, input),
            Self::Douyin(resolver) => PlatformResolver::extract_share_url(resolver, input),
        }
    }

    pub async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        match self {
            Self::Wechat(resolver) => PlatformResolver::resolve_text(resolver, input).await,
            Self::Douyin(resolver) => PlatformResolver::resolve_text(resolver, input).await,
        }
    }

    pub async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        match self {
            Self::Wechat(resolver) => PlatformResolver::resolve_url(resolver, url).await,
            Self::Douyin(resolver) => PlatformResolver::resolve_url(resolver, url).await,
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
}

/// Static extract without a [`crate::ParseKit`] instance.
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
    fn extract_share_url_rejects_unknown() {
        let err = extract_share_url("https://www.example.com/video/1")
            .expect_err("unknown host should be unsupported");
        assert!(matches!(err, Error::UnsupportedUrl));
    }

    #[test]
    fn default_match_order_is_wechat_then_douyin() {
        assert_eq!(STATELESS_EXTRACTORS.len(), 2);
        // Ambiguous free text is not expected; order is documented for hub registration.
        let wechat = wechat::extract_share_url("https://weixin.qq.com/sph/A27pGwf5f9").unwrap();
        let douyin = douyin::extract_share_url("https://v.douyin.com/iAbCdEf/").unwrap();
        assert!(wechat.as_str().contains("weixin"));
        assert!(douyin.as_str().contains("douyin"));
    }
}
