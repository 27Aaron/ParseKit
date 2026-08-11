use parse_kit::platforms::DouyinResolver;

const SAMPLE_SHARE: &str = "https://v.douyin.com/q75E3VmAe6A/";

#[tokio::test]
#[ignore = "requires network access to Douyin"]
async fn resolves_douyin_sample_url() {
    let resolver = DouyinResolver::new().expect("douyin resolver");
    let post = resolver
        .resolve_text(SAMPLE_SHARE)
        .await
        .unwrap_or_else(|error| panic!("douyin resolve failed: {error}"));

    assert_eq!(post.platform, parse_kit::PlatformId::Douyin);
    assert_eq!(post.post_id, "7661946724177829115");
    assert_eq!(post.primary_video().unwrap().url.scheme(), "https");
    assert!(
        post.primary_video().unwrap().url.host_str().is_some(),
        "resolved video URL missing host"
    );
    println!("resolved post_id={} title={:?}", post.post_id, post.title);
}
