use parse_kit::{PlatformId, bilibili::BilibiliResolver};

const SAMPLE_SHARE: &str = "https://www.bilibili.com/video/BV1GJ411x7h7";

#[tokio::test]
#[ignore = "requires network access to Bilibili"]
async fn resolves_bilibili_sample_url() {
    let resolver = BilibiliResolver::new().expect("bilibili resolver");
    let post = resolver
        .resolve_text(SAMPLE_SHARE)
        .await
        .expect("bilibili resolve");
    assert_eq!(post.platform, PlatformId::Bilibili);
    assert!(!post.post_id.is_empty());
    let video = post
        .primary_video()
        .expect("expected at least one video source");
    assert_eq!(video.url.scheme(), "https");
    assert!(video.url.host_str().is_some());
}
