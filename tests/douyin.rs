use parse_kit::{
    Error,
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
async fn reports_browser_verification_requirement_without_network_access() {
    let resolver = DouyinResolver::new().expect("douyin resolver");
    let error = resolver
        .resolve_text(SAMPLE_SHARE)
        .await
        .expect_err("Douyin resolution is intentionally paused");

    assert!(matches!(error, Error::PlatformUnavailable(_)));
    assert!(error.to_string().contains("浏览器验证"));
}
