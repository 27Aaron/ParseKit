use parse_kit::{
    PlatformId,
    platforms::{DouyinResolver, douyin::extract_share_url},
};

const SAMPLE_SHARE: &str = "https://v.douyin.com/q75E3VmAe6A/";
const SAMPLE_VIDEO: &str = "https://www.douyin.com/video/7661946724177829115";

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
        .resolve_text(SAMPLE_VIDEO)
        .await
        .expect("douyin resolve");
    assert_eq!(post.platform, PlatformId::Douyin);
    assert!(!post.post_id.is_empty());
    let source = post
        .primary_video()
        .or_else(|| post.media_sources().next())
        .expect("expected at least one media source");
    assert_eq!(source.url.scheme(), "https");
    assert!(source.url.host_str().is_some());
}
