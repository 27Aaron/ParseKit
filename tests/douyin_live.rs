//! Live Douyin resolver tests, ignored by default.
//!
//! Opt in with a public video share link:
//!
//! ```bash
//! DOUYIN_SAMPLE_URL='https://v.douyin.com/AbCdEf' \
//!   cargo test --test douyin_live -- --ignored --nocapture
//! ```

use std::env;

use parse_kit::platforms::DouyinResolver;

#[tokio::test]
#[ignore = "requires DOUYIN_SAMPLE_URL and live Douyin endpoints"]
async fn resolves_douyin_sample_url() {
    let sample =
        env::var("DOUYIN_SAMPLE_URL").expect("set DOUYIN_SAMPLE_URL to a public Douyin share URL");
    let resolver = DouyinResolver::new().expect("douyin resolver");
    let post = resolver
        .resolve_text(&sample)
        .await
        .unwrap_or_else(|error| panic!("douyin resolve failed: {error}"));

    assert_eq!(post.platform, parse_kit::PlatformId::Douyin);
    assert!(!post.post_id.is_empty());
    assert_eq!(post.primary_video().unwrap().url.scheme(), "https");
    assert!(
        post.primary_video().unwrap().url.host_str().is_some(),
        "resolved video URL missing host"
    );
    println!("resolved post_id={} title={:?}", post.post_id, post.title);
}
