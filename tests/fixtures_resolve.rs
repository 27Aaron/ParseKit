//! CI smoke: committed fixtures parse and keep expected shape.

#[test]
fn wechat_feed_fixture_is_valid_json_with_media_urls() {
    let feed: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/wechat/feed_h264_preferred.json")).unwrap();
    assert_eq!(feed.get("errCode").and_then(|v| v.as_i64()), Some(0));
    let h264 = feed
        .pointer("/data/feedInfo/h264VideoInfo/videoUrl")
        .and_then(|v| v.as_str())
        .expect("h264 url");
    let url = url::Url::parse(h264).unwrap();
    let direct = parse_kit::wechat::derive_direct_media_url(&url).expect("derive");
    assert_eq!(direct.query(), Some("encfilekey=h&token=t"));
}

#[test]
fn douyin_router_fixture_is_valid_json_with_item() {
    let router: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/douyin/router_video.json")).unwrap();
    let item = router
        .pointer("/loaderData/video_(id)~1page/videoInfoRes/item_list/0")
        .expect("item");
    assert_eq!(
        item.get("aweme_id").and_then(|v| v.as_str()),
        Some("7123456789012345678")
    );
    let play = item
        .pointer("/video/play_addr/url_list/0")
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(play.contains("playwm"));
    assert!(play.replace("/playwm/", "/play/").contains("/play/"));
}

#[test]
fn douyin_html_fixture_contains_router_marker() {
    let html = include_str!("fixtures/douyin/share_page_router.html");
    assert!(html.contains("window._ROUTER_DATA"));
    assert!(html.contains("videoInfoRes"));
}

#[test]
fn bilibili_fixtures_build_post() {
    let view: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/bilibili/view.json")).unwrap();
    let play: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/bilibili/playurl.json")).unwrap();
    let post = parse_kit::bilibili::build_post_from_fixtures(&view, &play).unwrap();
    assert_eq!(post.platform, parse_kit::PlatformId::Bilibili);
    assert!(post.primary_video().is_some());
}
