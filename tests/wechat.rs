use std::{env, path::PathBuf, time::Duration};

use parse_kit::{
    PlatformId,
    media::{DownloadRequestIdentity, MediaDownloader, probe_media},
    model::{MediaSource, MediaSourceKind, ResolvedPost},
    wechat::{REVIEWED_WECHAT_MEDIA_HOSTS, WechatResolver, extract_share_url},
};
use uuid::Uuid;

const SAMPLE_SHARE_URL: &str = "https://weixin.qq.com/sph/AzJ7CGPYWD";
const LIVE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(20 * 60);

#[test]
fn extracts_wechat_share_text_without_network_access() {
    let url = extract_share_url(&format!("看看这个 {SAMPLE_SHARE_URL}】"))
        .expect("WeChat URL extraction");

    assert_eq!(url.as_str(), SAMPLE_SHARE_URL);
}

#[tokio::test]
#[ignore = "requires YUANBAO_COOKIE and live Tencent endpoints"]
async fn resolves_wechat_sample() {
    let post = resolve_sample().await;

    assert_eq!(post.platform, PlatformId::Wechat);
    assert_eq!(post.canonical_url.as_str(), SAMPLE_SHARE_URL);
    assert!(!post.post_id.trim().is_empty(), "missing post identifier");
    let primary = post.primary_video().expect("resolved post has no video");
    assert_safe_media_source(primary);

    if primary.provenance == MediaSourceKind::Derived {
        let query: Vec<_> = primary.url.query_pairs().collect();
        assert_eq!(query.len(), 2, "derived source query shape changed");
        assert!(
            query
                .iter()
                .any(|(key, value)| key == "encfilekey" && !value.is_empty()),
            "derived source is missing encfilekey"
        );
        assert!(
            query
                .iter()
                .any(|(key, value)| key == "token" && !value.is_empty()),
            "derived source is missing token"
        );
    }
}

#[tokio::test]
#[ignore = "downloads live Tencent media and requires YUANBAO_COOKIE plus ffprobe"]
async fn downloads_decrypts_and_probes_wechat_sample() {
    let post = resolve_sample().await;
    let directory = TestDirectory::new();
    let downloader = MediaDownloader::with_options(
        directory.path(),
        REVIEWED_WECHAT_MEDIA_HOSTS.iter().copied(),
        LIVE_DOWNLOAD_TIMEOUT,
        DownloadRequestIdentity::wechat(),
    )
    .unwrap_or_else(|_| panic!("failed to initialize the live media downloader"));
    let downloaded = downloader
        .download_playable(post.media_sources())
        .await
        .unwrap_or_else(|_| panic!("live WeChat media download failed"));

    assert!(downloaded.bytes > 0, "downloaded media is empty");
    let probe = probe_media(&downloaded.path)
        .await
        .unwrap_or_else(|_| panic!("live WeChat media probe failed"));
    assert!(
        probe.width > 0 && probe.height > 0,
        "invalid video dimensions"
    );

    let path = downloaded.path.clone();
    downloaded
        .cleanup()
        .await
        .unwrap_or_else(|_| panic!("failed to clean up live test media"));
    assert!(!path.exists(), "live test media was not removed");
}

async fn resolve_sample() -> ResolvedPost {
    let _ = dotenvy::from_filename(".env.local");
    let _ = dotenvy::dotenv();
    let cookie = match env::var("YUANBAO_COOKIE") {
        Ok(cookie) if !cookie.trim().is_empty() => cookie,
        Ok(_) | Err(env::VarError::NotPresent) => {
            panic!("YUANBAO_COOKIE is required for this ignored live test")
        }
        Err(env::VarError::NotUnicode(_)) => {
            panic!("YUANBAO_COOKIE must contain valid Unicode")
        }
    };

    let resolver = WechatResolver::new(cookie)
        .unwrap_or_else(|_| panic!("failed to initialize the WeChat resolver"));
    resolver
        .resolve_text(SAMPLE_SHARE_URL)
        .await
        .unwrap_or_else(|_| panic!("live WeChat resolution failed"))
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "parse-kit-wechat-live-{}",
            Uuid::new_v4().hyphenated()
        ));
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|_| panic!("failed to create live test directory"));
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_safe_media_source(source: &MediaSource) {
    assert!(
        source.url.scheme() == "https",
        "media source must use HTTPS"
    );
    assert!(
        source.url.username().is_empty()
            && source.url.password().is_none()
            && source.url.port() != Some(0),
        "media source contains unexpected authority components"
    );
    assert!(
        source
            .url
            .host_str()
            .is_some_and(|host| REVIEWED_WECHAT_MEDIA_HOSTS.contains(&host)),
        "media source host is not allowlisted"
    );
}
