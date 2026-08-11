use url::Url;

use crate::{
    Error, ResolvedPost, Result,
    platforms::{self, DouyinResolver, Platform, WechatResolver},
};

/// Multi-platform parse facade.
///
/// Delivery apps (bots, CLI, …) depend on this type rather than individual
/// platform resolvers so new platforms can be registered here without touching
/// product code.
///
/// Platforms are tried in registration order (see [`ParseKitBuilder`]).
#[derive(Debug, Clone)]
pub struct ParseKit {
    platforms: Vec<Platform>,
}

/// Builds a [`ParseKit`] with an explicit set of platforms.
///
/// Default match order when using [`ParseKit::new`] is WeChat Channels, then
/// Douyin — the same order as [`platforms::STATELESS_EXTRACTORS`].
#[derive(Debug, Default)]
pub struct ParseKitBuilder {
    platforms: Vec<Platform>,
}

impl ParseKitBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register WeChat Channels (requires Yuanbao cookie).
    pub fn wechat(mut self, yuanbao_cookie: impl Into<String>) -> Result<Self> {
        self.platforms
            .push(Platform::Wechat(WechatResolver::new(yuanbao_cookie)?));
        Ok(self)
    }

    /// Register Douyin (no cookie required for the public share-page path).
    pub fn douyin(mut self) -> Result<Self> {
        self.platforms
            .push(Platform::Douyin(DouyinResolver::new()?));
        Ok(self)
    }

    /// Register an already-constructed platform backend.
    pub fn platform(mut self, platform: Platform) -> Self {
        self.platforms.push(platform);
        self
    }

    /// Finish building. At least one platform must be registered.
    pub fn build(self) -> Result<ParseKit> {
        if self.platforms.is_empty() {
            return Err(Error::Config("至少注册一个解析平台".into()));
        }
        Ok(ParseKit {
            platforms: self.platforms,
        })
    }
}

impl ParseKit {
    /// Build a kit with the platforms currently supported by this build.
    ///
    /// Registration order is match order for share text and URLs:
    /// WeChat Channels, then Douyin (aligned with [`platforms::STATELESS_EXTRACTORS`]).
    ///
    /// For Douyin-only (or custom sets), use [`ParseKit::builder`].
    pub fn new(wechat_yuanbao_cookie: impl Into<String>) -> Result<Self> {
        Self::builder()
            .wechat(wechat_yuanbao_cookie)?
            .douyin()?
            .build()
    }

    /// Start a custom platform registration (optional WeChat / Douyin / …).
    pub fn builder() -> ParseKitBuilder {
        ParseKitBuilder::new()
    }

    /// Extract a supported share URL from free-form text, or reject early.
    ///
    /// Uses the same platform matcher order as a fully constructed kit, but
    /// does not require cookies or network clients.
    pub fn extract_share_url(input: &str) -> Result<Url> {
        platforms::extract_share_url(input)
    }

    pub async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        for platform in &self.platforms {
            match platform.extract_share_url(input) {
                Ok(_) => return platform.resolve_text(input).await,
                Err(Error::UnsupportedUrl) => {}
                Err(error) => return Err(error),
            }
        }
        Err(Error::UnsupportedUrl)
    }

    pub async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        for platform in &self.platforms {
            match platform.extract_share_url(url.as_str()) {
                Ok(_) => return platform.resolve_url(url).await,
                Err(Error::UnsupportedUrl) => {}
                Err(error) => return Err(error),
            }
        }
        Err(Error::UnsupportedUrl)
    }

    /// Access the WeChat resolver when it was registered.
    pub fn wechat(&self) -> Option<&WechatResolver> {
        self.platforms.iter().find_map(Platform::as_wechat)
    }

    /// Access the Douyin resolver when it was registered.
    pub fn douyin(&self) -> Option<&DouyinResolver> {
        self.platforms.iter().find_map(Platform::as_douyin)
    }

    /// Registered platforms in match order (for tests / introspection).
    pub fn platforms(&self) -> &[Platform] {
        &self.platforms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_requires_at_least_one_platform() {
        let err = ParseKit::builder().build().unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn builder_can_register_douyin_only() {
        let hub = ParseKit::builder().douyin().unwrap().build().unwrap();
        assert!(hub.wechat().is_none());
        assert!(hub.douyin().is_some());
        assert_eq!(hub.platforms().len(), 1);
    }

    #[test]
    fn new_registers_wechat_then_douyin() {
        let hub = ParseKit::new("hy_user=test; token=test").unwrap();
        assert!(hub.wechat().is_some());
        assert!(hub.douyin().is_some());
        assert_eq!(
            hub.platforms()
                .iter()
                .map(Platform::platform_id)
                .collect::<Vec<_>>(),
            ["wechat_channels", "douyin"]
        );
    }
}
