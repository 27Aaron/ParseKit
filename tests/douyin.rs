use parse_kit::{
    PlatformId,
    platforms::{DouyinResolver, douyin::extract_share_url},
};

const SAMPLE_SHARE: &str = "https://v.douyin.com/q75E3VmAe6A/";

#[test]
fn extracts_and_upgrades_douyin_share_url_without_network_access() {
    let url = extract_share_url("复制 http://v.douyin.com/q75E3VmAe6A/。")
        .expect("Douyin URL extraction");

    assert_eq!(url.as_str(), SAMPLE_SHARE);
}

#[tokio::test]
#[ignore = "requires network access to Douyin"]
async fn resolves_douyin_sample_url() {
    let resolver = DouyinResolver::new().expect("douyin resolver");
    let post = resolver
        .resolve_text(SAMPLE_SHARE)
        .await
        .unwrap_or_else(|error| panic!("douyin resolve failed: {error}"));

    assert_eq!(post.platform, PlatformId::Douyin);
    assert_eq!(post.post_id, "7661946724177829115");
    let primary = post.primary_video().expect("expected a video source");
    assert_eq!(primary.url.scheme(), "https");
    assert!(
        primary.url.host_str().is_some(),
        "resolved video URL missing host"
    );
}
