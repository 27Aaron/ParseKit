//! Bilibili resolver unit tests.

use super::{
    parse::{build_post_from_payloads, collect_play_sources},
    *,
};

#[test]
fn extracts_bv_av_mobile_and_short_urls() {
    let bv = extract_share_url("看 https://www.bilibili.com/video/BV1GJ411x7h7?spm=1").unwrap();
    assert!(bv.path().contains("BV1GJ411x7h7"));

    let av = extract_share_url("https://m.bilibili.com/video/av170001").unwrap();
    assert!(av.as_str().contains("av170001"));

    let short = extract_share_url("http://b23.tv/abc123").unwrap();
    assert_eq!(short.scheme(), "https");
}

#[test]
fn rejects_unrelated() {
    assert!(matches!(
        extract_share_url("https://www.example.com/video/BV1xx"),
        Err(Error::UnsupportedUrl)
    ));
}

#[test]
fn builds_from_committed_fixtures() {
    let view: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/bilibili/view.json"))
            .expect("committed Bilibili view fixture");
    let play: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/bilibili/playurl.json"
    ))
    .expect("committed Bilibili playurl fixture");

    let post = build_post_from_payloads(&view, &play).unwrap();
    assert_eq!(post.platform, PlatformId::Bilibili);
    assert_eq!(post.title.as_deref(), Some("测试稿件"));
    let primary = post.primary_video().expect("primary video");
    assert!(primary.url.as_str().contains("bilivideo.com"));
    assert_eq!(primary.size_hint, Some(12345));
}

#[test]
fn collects_dash_video_by_bandwidth() {
    let play = serde_json::json!({
        "dash": {
            "video": [
                {
                    "bandwidth": 500_000,
                    "width": 640,
                    "height": 360,
                    "base_url": "https://upos-sz-mirrorcos.bilivideo.com/low.m4s"
                },
                {
                    "bandwidth": 2_000_000,
                    "width": 1920,
                    "height": 1080,
                    "baseUrl": "https://upos-sz-mirrorhw.bilivideo.com/high.m4s",
                    "backup_url": ["https://upos-sz-mirrorali.bilivideo.com/high-b.m4s"]
                }
            ]
        }
    });
    let sources = collect_play_sources(&play);
    assert!(sources.len() >= 2);
    assert!(sources[0].url.as_str().contains("high"));
    assert_eq!(sources[0].width, Some(1920));
}

#[test]
fn rejects_lookalike_media_hosts() {
    let play = serde_json::json!({
        "durl": [{
            "url": "https://evilbilivideo.com/video.mp4",
            "size": 123
        }]
    });
    assert!(collect_play_sources(&play).is_empty());
}

#[test]
fn multi_part_durl_is_not_treated_as_fallback_sources() {
    let play = serde_json::json!({
        "durl": [
            {"url": "https://v1.bilivideo.com/part-1.mp4"},
            {"url": "https://v1.bilivideo.com/part-2.mp4"}
        ]
    });
    assert!(collect_play_sources(&play).is_empty());
}
