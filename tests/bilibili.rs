use parse_kit::{
    PlatformId,
    bilibili::{BilibiliResolver, extract_share_url},
};

const SAMPLE_SHARE: &str = "https://www.bilibili.com/video/BV1GJ411x7h7";
const MULTIPART_SHARE: &str = "https://www.bilibili.com/video/BV1Eb411u7Fw";

#[test]
fn extracts_bilibili_share_text_without_network_access() {
    let url = extract_share_url(&format!("看看 {SAMPLE_SHARE}?utm_source=copy 这个"))
        .expect("Bilibili URL extraction");

    assert_eq!(url.as_str(), SAMPLE_SHARE);
}

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

#[tokio::test]
#[ignore = "requires network access to Bilibili"]
async fn resolves_the_requested_multipart_page() {
    let resolver = BilibiliResolver::new().expect("bilibili resolver");
    let first = resolver
        .resolve_text(&format!("{MULTIPART_SHARE}?p=1"))
        .await
        .expect("Bilibili P1 resolve");
    let second = resolver
        .resolve_text(&format!("{MULTIPART_SHARE}?p=2"))
        .await
        .expect("Bilibili P2 resolve");

    assert_eq!(
        second.canonical_url.as_str(),
        format!("{MULTIPART_SHARE}?p=2")
    );
    assert_eq!(second.download_file_stem(), "Bilibili_BV1Eb411u7Fw_p2");
    assert_ne!(
        first.primary_video().expect("P1 video").url.path(),
        second.primary_video().expect("P2 video").url.path(),
        "different pages must not resolve the same cid"
    );
}
