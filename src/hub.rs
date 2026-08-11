use std::path::PathBuf;

use url::Url;

use crate::{
    Error, ResolvedPost, Result,
    media::MediaDownloader,
    platforms::{self, DouyinResolver, Platform, WechatResolver},
};

/// Multi-platform facade; tries platforms in registration order.
#[derive(Debug, Clone)]
pub struct ParseKit {
    platforms: Vec<Platform>,
}

/// Kit builder. [`ParseKit::new`] registers WeChat then Douyin.
#[derive(Debug, Default)]
pub struct ParseKitBuilder {
    platforms: Vec<Platform>,
}

impl ParseKitBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn wechat(mut self, yuanbao_cookie: impl Into<String>) -> Result<Self> {
        self.platforms
            .push(Platform::Wechat(WechatResolver::new(yuanbao_cookie)?));
        Ok(self)
    }

    pub fn douyin(mut self) -> Result<Self> {
        self.platforms
            .push(Platform::Douyin(DouyinResolver::new()?));
        Ok(self)
    }

    pub fn platform(mut self, platform: Platform) -> Self {
        self.platforms.push(platform);
        self
    }

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
    /// WeChat + Douyin. Custom sets: [`ParseKit::builder`].
    pub fn new(wechat_yuanbao_cookie: impl Into<String>) -> Result<Self> {
        Self::builder()
            .wechat(wechat_yuanbao_cookie)?
            .douyin()?
            .build()
    }

    pub fn builder() -> ParseKitBuilder {
        ParseKitBuilder::new()
    }

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

    pub fn wechat(&self) -> Option<&WechatResolver> {
        self.platforms.iter().find_map(Platform::as_wechat)
    }

    pub fn douyin(&self) -> Option<&DouyinResolver> {
        self.platforms.iter().find_map(Platform::as_douyin)
    }

    pub fn platforms(&self) -> &[Platform] {
        &self.platforms
    }

    /// Downloader allowlisted for `post.platform`.
    pub fn media_downloader_for(
        &self,
        post: &ResolvedPost,
        workspace_dir: impl Into<PathBuf>,
        max_bytes: u64,
    ) -> Result<MediaDownloader> {
        MediaDownloader::for_platform(post.platform.as_str(), workspace_dir, max_bytes)
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

    #[test]
    fn media_downloader_for_selects_platform_allowlist() {
        use url::Url;

        use crate::model::{MediaSource, MediaSourceKind, ResolvedPost, VideoCodec};

        let kit = ParseKit::builder().douyin().unwrap().build().unwrap();
        let post = ResolvedPost::new_video(
            "douyin",
            "1",
            Url::parse("https://www.douyin.com/video/1").unwrap(),
            None,
            None,
            MediaSource {
                url: Url::parse("https://aweme.snssdk.com/play").unwrap(),
                codec: VideoCodec::Unknown,
                provenance: MediaSourceKind::Direct,
                width: None,
                height: None,
                size_hint: None,
                decode_key: None,
            },
            Vec::new(),
        );
        let downloader = kit
            .media_downloader_for(&post, "/tmp/parse-kit-test", 1024)
            .unwrap();
        assert!(
            downloader
                .allowed_hosts()
                .iter()
                .any(|h| { h.contains("douyin") || h.contains("snssdk") || h.starts_with('.') })
        );
    }
}
