use std::path::PathBuf;

use tracing::Instrument;
use url::Url;

use crate::{
    Error, ResolvedPost, Result,
    media::MediaDownloader,
    platforms::{self, BilibiliResolver, DouyinResolver, Platform, WechatResolver},
};

/// Resolves posts through registered platforms in order.
#[derive(Debug, Clone)]
pub struct ParseKit {
    platforms: Vec<Platform>,
}

/// Configures the platform resolver order used by [`ParseKit`].
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

    pub fn bilibili(mut self) -> Result<Self> {
        self.platforms
            .push(Platform::Bilibili(BilibiliResolver::new()?));
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
    /// Builds the default WeChat, Douyin, and Bilibili resolver set.
    ///
    /// Use [`ParseKit::builder`] to select a custom set or order.
    pub fn new(wechat_yuanbao_cookie: impl Into<String>) -> Result<Self> {
        Self::builder()
            .wechat(wechat_yuanbao_cookie)?
            .douyin()?
            .bilibili()?
            .build()
    }

    pub fn builder() -> ParseKitBuilder {
        ParseKitBuilder::new()
    }

    pub fn extract_share_url(input: &str) -> Result<Url> {
        platforms::extract_share_url(input)
    }

    pub async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        let span = tracing::info_span!("parse_kit.resolve_text");
        async move {
            let platform = self.matching_platform(input)?;
            let id = platform.platform_id().as_str();
            platform
                .resolve_text(input)
                .instrument(tracing::info_span!("platform.resolve", platform = id))
                .await
        }
        .instrument(span)
        .await
    }

    pub async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        let span = tracing::info_span!("parse_kit.resolve_url");
        async move {
            let platform = self.matching_platform(url.as_str())?;
            let id = platform.platform_id().as_str();
            platform
                .resolve_url(url)
                .instrument(tracing::info_span!("platform.resolve", platform = id))
                .await
        }
        .instrument(span)
        .await
    }

    fn matching_platform(&self, input: &str) -> Result<&Platform> {
        for platform in &self.platforms {
            match platform.extract_share_url(input) {
                Ok(_) => return Ok(platform),
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

    pub fn bilibili(&self) -> Option<&BilibiliResolver> {
        self.platforms.iter().find_map(Platform::as_bilibili)
    }

    pub fn platforms(&self) -> &[Platform] {
        &self.platforms
    }

    /// Creates a downloader for the post's platform hosts and file stem.
    pub fn media_downloader_for(
        &self,
        post: &ResolvedPost,
        workspace_dir: impl Into<PathBuf>,
    ) -> Result<MediaDownloader> {
        Ok(MediaDownloader::for_platform(post.platform, workspace_dir)?
            .with_file_stem(post.download_file_stem()))
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
    fn new_registers_wechat_douyin_bilibili() {
        let hub = ParseKit::new("hy_user=test; token=test").unwrap();
        assert!(hub.wechat().is_some());
        assert!(hub.douyin().is_some());
        assert_eq!(
            hub.platforms()
                .iter()
                .map(Platform::platform_id)
                .collect::<Vec<_>>(),
            [
                crate::PlatformId::Wechat,
                crate::PlatformId::Douyin,
                crate::PlatformId::Bilibili,
            ]
        );
    }

    #[test]
    fn media_downloader_for_selects_platform_allowlist() {
        use url::Url;

        use crate::model::{MediaSource, MediaSourceKind, ResolvedPost, VideoCodec};

        let kit = ParseKit::builder().douyin().unwrap().build().unwrap();
        let post = ResolvedPost::new_video(
            crate::PlatformId::Douyin,
            "7661946724177829115",
            Url::parse("https://www.douyin.com/video/7661946724177829115").unwrap(),
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
                label: None,
                bitrate_bps: None,
            },
            Vec::new(),
        );
        let downloader = kit
            .media_downloader_for(&post, "/tmp/parse-kit-test")
            .unwrap();
        assert!(
            downloader
                .allowed_hosts()
                .iter()
                .any(|h| { h.contains("douyin") || h.contains("snssdk") || h.starts_with('.') })
        );
        assert_eq!(downloader.file_stem(), Some("Douyin_7661946724177829115"));
    }

    #[test]
    fn matching_platform_routes_using_registered_resolvers() {
        let kit = ParseKit::builder().bilibili().unwrap().build().unwrap();
        let platform = kit
            .matching_platform("看看 https://www.bilibili.com/video/BV1GJ411x7h7?utm_source=chat")
            .expect("Bilibili match");

        assert_eq!(platform.platform_id(), crate::PlatformId::Bilibili);
    }
}
