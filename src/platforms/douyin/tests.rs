use super::{
    extract_share_url,
    parse::{
        build_post_from_router, parse_any_page_data, parse_router_data, remove_video_watermark,
    },
    share::{extract_aweme_id, is_allowed_redirect_host},
};
use crate::{Error, PlatformId};
use url::Url;

#[test]
fn extracts_urls_from_share_text() {
    let cases = [
        (
            "5.64 08/07 dAG:/ t@R.KJ :0pm 【梁山伯_】 快醒醒呀兄弟们！# 梁山伯_ # 直播整活 # 雷姆 # 直播录屏分享  https://v.douyin.com/q75E3VmAe6A/ 复制此链接，打开Dou音搜索，直接观看视频！",
            "https://v.douyin.com/q75E3VmAe6A/",
        ),
        (
            "https://www.douyin.com/video/7661946724177829115",
            "https://www.douyin.com/video/7661946724177829115",
        ),
        (
            "https://www.iesdouyin.com/share/video/7661946724177829115/",
            "https://www.iesdouyin.com/share/video/7661946724177829115/",
        ),
    ];
    for (input, expected) in cases {
        let url = extract_share_url(input).expect(input);
        assert_eq!(url.as_str(), expected);
    }
}

#[test]
fn rejects_non_douyin_and_user_paths() {
    assert!(matches!(
        extract_share_url("https://www.example.com/video/1"),
        Err(Error::UnsupportedUrl)
    ));
    assert!(matches!(
        extract_share_url("https://www.douyin.com/share/user/123"),
        Err(Error::UnsupportedUrl)
    ));
    assert!(matches!(
        extract_share_url("https://www.douyin.com/user/self"),
        Err(Error::UnsupportedUrl)
    ));
    assert!(matches!(
        extract_share_url("https://www.douyin.com:8443/video/12345"),
        Err(Error::UnsupportedUrl)
    ));
}

#[test]
fn redirect_hosts_reject_unsafe_authorities() {
    assert!(is_allowed_redirect_host(
        &Url::parse("https://www.douyin.com/video/12345").unwrap()
    ));
    for raw in [
        "http://www.douyin.com/video/12345",
        "https://user@www.douyin.com/video/12345",
        "https://www.douyin.com:8443/video/12345",
        "https://www.douyin.com./video/12345",
        "https://www.douyin.com.evil.test/video/12345",
    ] {
        assert!(
            !is_allowed_redirect_host(&Url::parse(raw).unwrap()),
            "{raw}"
        );
    }
}

#[test]
fn extracts_aweme_ids() {
    assert_eq!(
        extract_aweme_id("https://www.douyin.com/video/7661946724177829115?x=1").as_deref(),
        Some("7661946724177829115")
    );
    assert_eq!(
        extract_aweme_id("https://www.iesdouyin.com/share/video/7661946724177829115/").as_deref(),
        Some("7661946724177829115")
    );
    assert_eq!(
        extract_aweme_id("https://www.douyin.com/discover?modal_id=7661946724177829115").as_deref(),
        Some("7661946724177829115")
    );
    assert_eq!(
        extract_aweme_id("https://www.douyin.com/note/7661946724177829115").as_deref(),
        Some("7661946724177829115")
    );
}

#[test]
fn builds_post_from_fixture_item_list() {
    let router: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/douyin/router_video.json"
    ))
    .expect("committed douyin router fixture");

    let post = build_post_from_router("7661946724177829115", &router).unwrap();
    assert_eq!(post.platform, PlatformId::Douyin);
    assert_eq!(post.post_id, "7661946724177829115");
    assert_eq!(post.title.as_deref(), Some("测试标题"));
    let primary = post.primary_video().expect("primary video");
    assert!(primary.url.as_str().contains("play/?video_id="));
    assert!(!primary.url.as_str().contains("playwm"));
    assert_eq!(primary.width, Some(720));
    assert_eq!(primary.height, Some(1280));
    assert!(post.cover_url.is_some());
}

#[test]
fn parse_router_data_from_committed_html_fixture() {
    let html = include_str!("../../../tests/fixtures/douyin/share_page_router.html");
    let value = parse_router_data(html).unwrap();
    assert!(
        value
            .pointer("/loaderData/video_(id)~1page/videoInfoRes")
            .is_some()
    );
}

#[test]
fn filter_list_maps_to_not_found() {
    let router = serde_json::json!({
        "loaderData": {
            "video_(id)/page": {
                "videoInfoRes": {
                    "status_code": 0,
                    "filter_list": [{
                        "aweme_id": "1",
                        "filter_reason": "SYSTEM_ITEM_NOT_EXIST"
                    }],
                    "item_list": []
                }
            }
        }
    });
    let err = build_post_from_router("1", &router).unwrap_err();
    assert!(matches!(err, Error::NotFound));
}

#[test]
fn image_posts_become_image_set() {
    let router = serde_json::json!({
        "loaderData": {
            "video_(id)/page": {
                "videoInfoRes": {
                    "status_code": 0,
                    "filter_list": [],
                    "item_list": [{
                        "aweme_id": "1",
                        "desc": "图集",
                        "images": [
                            {"url_list": ["https://p3.douyinpic.com/a.jpg"], "width": 1080, "height": 1440},
                            {"url_list": ["https://p3.douyinpic.com/b.jpg"]}
                        ],
                        "video": {}
                    }]
                }
            }
        }
    });
    let post = build_post_from_router("1", &router).unwrap();
    assert_eq!(post.kind, crate::ContentKind::ImageSet);
    assert_eq!(post.media_sources().count(), 2);
    assert!(post.primary_video().is_none());
    assert!(
        post.media_sources()
            .next()
            .expect("first image")
            .url
            .as_str()
            .contains("douyinpic.com")
    );
}

#[test]
fn unreviewed_media_hosts_are_discarded() {
    let router = serde_json::json!({
        "loaderData": {
            "video_(id)/page": {
                "videoInfoRes": {
                    "filter_list": [],
                    "item_list": [{
                        "aweme_id": "9",
                        "video": {"play_addr": {"url_list": ["https://evil.test/play.mp4"]}}
                    }]
                }
            }
        }
    });

    assert!(matches!(
        build_post_from_router("9", &router),
        Err(Error::MediaUnavailable)
    ));
}

#[test]
fn watermark_cleanup_only_changes_the_path_segment() {
    let url =
        Url::parse("https://aweme.snssdk.com/aweme/v1/playwm/?video_id=playwm-token&note=playwm")
            .expect("test URL");
    let cleaned = remove_video_watermark(url);

    assert_eq!(cleaned.path(), "/aweme/v1/play/");
    assert_eq!(cleaned.query(), Some("video_id=playwm-token&note=playwm"));
}

#[test]
fn multi_bitrate_becomes_fallbacks() {
    let router = serde_json::json!({
        "loaderData": {
            "video_(id)/page": {
                "videoInfoRes": {
                    "status_code": 0,
                    "filter_list": [],
                    "item_list": [{
                        "aweme_id": "9",
                        "desc": "多清晰度",
                        "video": {
                            "bit_rate": [
                                {
                                    "bit_rate": 500_000,
                                    "play_addr": {
                                        "url_list": ["https://aweme.snssdk.com/aweme/v1/play/?video_id=low"],
                                        "width": 720,
                                        "height": 1280,
                                        "data_size": 1_000
                                    }
                                },
                                {
                                    "bit_rate": 2_000_000,
                                    "play_addr": {
                                        "url_list": ["https://aweme.snssdk.com/aweme/v1/play/?video_id=high"],
                                        "width": 1080,
                                        "height": 1920,
                                        "data_size": 5_000
                                    }
                                }
                            ]
                        }
                    }]
                }
            }
        }
    });
    let post = build_post_from_router("9", &router).unwrap();
    assert!(
        post.primary_video()
            .expect("primary video")
            .url
            .as_str()
            .contains("video_id=high")
    );
    assert_eq!(post.video_fallbacks().len(), 1);
    assert!(
        post.video_fallbacks()[0]
            .url
            .as_str()
            .contains("video_id=low")
    );
}

#[test]
fn resolution_outranks_an_unrelated_large_file_size() {
    let router = serde_json::json!({
        "loaderData": {"video_(id)/page": {"videoInfoRes": {
            "filter_list": [],
            "item_list": [{
                "aweme_id": "9",
                "video": {"bit_rate": [
                    {
                        "bit_rate": 500_000,
                        "play_addr": {
                            "url_list": ["https://aweme.snssdk.com/low.mp4"],
                            "width": 720,
                            "height": 1280,
                            "data_size": 9_000_000_000_u64
                        }
                    },
                    {
                        "bit_rate": 2_000_000,
                        "play_addr": {
                            "url_list": ["https://aweme.snssdk.com/high.mp4"],
                            "width": 1080,
                            "height": 1920,
                            "data_size": 1_000
                        }
                    }
                ]}
            }]
        }}}
    });
    let post = build_post_from_router("9", &router).unwrap();
    assert!(
        post.primary_video()
            .unwrap()
            .url
            .path()
            .ends_with("high.mp4")
    );
}

#[test]
fn uri_fallback_is_query_encoded() {
    let router = serde_json::json!({
        "loaderData": {"video_(id)/page": {"videoInfoRes": {
            "filter_list": [],
            "item_list": [{
                "aweme_id": "9",
                "video": {"play_addr": {"uri": "opaque&id=unexpected"}}
            }]
        }}}
    });
    let post = build_post_from_router("9", &router).unwrap();
    let url = &post.primary_video().unwrap().url;
    assert_eq!(
        url.query_pairs()
            .find(|(key, _)| key == "video_id")
            .map(|(_, value)| value.into_owned())
            .as_deref(),
        Some("opaque&id=unexpected")
    );
    assert_eq!(url.query_pairs().filter(|(key, _)| key == "id").count(), 0);
}

#[test]
fn parse_router_data_from_html_snippet() {
    let html = r#"<!doctype html><html><body>
<script>window._ROUTER_DATA = {"loaderData":{"video_(id)/page":{"videoInfoRes":{"item_list":[],"filter_list":[],"status_code":0}}}};</script>
</body></html>"#;
    let value = parse_router_data(html).unwrap();
    assert!(
        value
            .pointer("/loaderData/video_(id)~1page/videoInfoRes")
            .is_some()
    );
}

#[test]
fn parse_any_page_data_accepts_render_data_marker() {
    let html = r#"<!doctype html><script>window.__RENDER_DATA__ = {"loaderData":{"video_(id)/page":{"videoInfoRes":{"item_list":[],"filter_list":[],"status_code":0}}}};</script>"#;
    let value = parse_any_page_data(html).unwrap();
    assert!(
        value
            .pointer("/loaderData/video_(id)~1page/videoInfoRes")
            .is_some()
    );
}

#[test]
fn parse_any_page_data_accepts_percent_encoded_json_and_skips_false_markers() {
    let html = r#"<script>const marker = "window.__RENDER_DATA__";</script>
<script>window.__RENDER_DATA__ = %7B%22loaderData%22%3A%7B%22video_(id)%2Fpage%22%3A%7B%22videoInfoRes%22%3A%7B%22item_list%22%3A%5B%5D%7D%7D%7D%7D;</script>"#;
    let value = parse_any_page_data(html).unwrap();
    assert!(
        value
            .pointer("/loaderData/video_(id)~1page/videoInfoRes")
            .is_some()
    );
}

#[test]
fn image_sources_fall_back_to_display_image_and_are_deduplicated() {
    let router = serde_json::json!({
        "loaderData": {"video_(id)/page": {"videoInfoRes": {
            "filter_list": [],
            "item_list": [{
                "aweme_id": "1",
                "images": [
                    {
                        "url_list": [],
                        "display_image": {"url_list": ["https://p3.douyinpic.com/a.jpg"]}
                    },
                    {"url_list": ["https://p3.douyinpic.com/a.jpg"]}
                ]
            }]
        }}}
    });
    let post = build_post_from_router("1", &router).unwrap();
    assert_eq!(post.media_sources().count(), 1);
}

#[test]
fn prefers_valid_origin_cover_when_cover_list_is_unusable() {
    let router = serde_json::json!({
        "loaderData": {"video_(id)/page": {"videoInfoRes": {
            "filter_list": [],
            "item_list": [{
                "aweme_id": "9",
                "video": {
                    "play_addr": {"url_list": ["https://aweme.snssdk.com/play.mp4"]},
                    "cover": {"url_list": ["https://evil.test/cover.jpg"]},
                    "origin_cover": {"url_list": ["https://p3.douyinpic.com/cover.jpg"]}
                }
            }]
        }}}
    });
    let post = build_post_from_router("9", &router).unwrap();
    assert_eq!(
        post.cover_url.as_ref().and_then(Url::host_str),
        Some("p3.douyinpic.com")
    );
}
