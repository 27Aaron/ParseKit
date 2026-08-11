//! Live Bilibili resolve tests (ignored by default).
//!
//! DOUYIN-style opt-in:
//! BILIBILI_SAMPLE_URL='https://www.bilibili.com/video/BVxxxx' \
//!   cargo test -p parse-kit --test bilibili_live -- --ignored --nocapture

use std::env;

use parse_kit::{PlatformId, bilibili::BilibiliResolver};

#[tokio::test]
#[ignore = "requires BILIBILI_SAMPLE_URL and live Bilibili endpoints"]
async fn resolves_bilibili_sample_url() {
    let sample = env::var("BILIBILI_SAMPLE_URL")
        .expect("set BILIBILI_SAMPLE_URL to a public bilibili video URL");
    let resolver = BilibiliResolver::new().expect("bilibili resolver");
    let post = resolver
        .resolve_text(&sample)
        .await
        .expect("live bilibili resolve");
    assert_eq!(post.platform, PlatformId::Bilibili);
    assert!(!post.post_id.is_empty());
    let video = post
        .primary_video()
        .expect("expected at least one video source");
    assert_eq!(video.url.scheme(), "https");
    assert!(video.url.host_str().is_some());
}
