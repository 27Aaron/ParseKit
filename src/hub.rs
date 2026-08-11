use url::Url;

use crate::{
    Error, ResolvedPost, Result,
    platforms::{self, Platform, WechatResolver},
};

/// Multi-platform resolve facade.
///
/// Delivery apps (Telegram bot, Feishu bot, CLI) depend on this type rather than
/// individual platform resolvers so new platforms can be registered here without
/// touching product code.
///
/// Platforms are tried in registration order (see [`ParseHub::new`]).
#[derive(Debug, Clone)]
pub struct ParseHub {
    platforms: Vec<Platform>,
}

impl ParseHub {
    /// Build a hub with the platforms currently supported by this build.
    ///
    /// Registration order is match order for share text and URLs.
    pub fn new(wechat_yuanbao_cookie: impl Into<String>) -> Result<Self> {
        Ok(Self {
            platforms: vec![Platform::Wechat(WechatResolver::new(
                wechat_yuanbao_cookie,
            )?)],
        })
    }

    /// Extract a supported share URL from free-form text, or reject early.
    ///
    /// Uses the same platform matcher order as a fully constructed hub, but
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

    /// Access the WeChat resolver for platform-specific configuration/tests.
    pub fn wechat(&self) -> &WechatResolver {
        self.platforms
            .iter()
            .find_map(Platform::as_wechat)
            .expect("WeChat Channels is always registered in ParseHub::new")
    }

    /// Registered platforms in match order (for tests / introspection).
    pub fn platforms(&self) -> &[Platform] {
        &self.platforms
    }
}
