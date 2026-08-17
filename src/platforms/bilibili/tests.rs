use super::{
    BilibiliResolver, extract_share_url,
    parse::{
        build_post_from_payloads, build_post_from_payloads_for_page, cid_from_view,
        collect_play_sources,
    },
    share::page_from_url,
};
use crate::{CredentialStatus, Error, PlatformId};

#[test]
fn extracts_bv_and_av_urls() {
    let bv = extract_share_url(
        "看 https://www.bilibili.com/video/BV1GJ411x7h7?p=2&spm_id_from=333.1007",
    )
    .unwrap();
    assert!(bv.path().contains("BV1GJ411x7h7"));
    assert_eq!(
        bv.query_pairs()
            .find(|(key, _)| key == "p")
            .map(|(_, value)| value.into_owned())
            .as_deref(),
        Some("2")
    );

    assert_eq!(page_from_url(&bv).unwrap(), Some(2));

    let av = extract_share_url("https://m.bilibili.com/video/av170001").unwrap();
    assert!(av.as_str().contains("av170001"));
}

#[test]
fn selects_and_names_requested_page() {
    let view = serde_json::json!({
        "bvid": "BV1GJ411x7h7",
        "aid": 170001,
        "title": "测试稿件",
        "cid": 111,
        "pages": [{"cid": 111}, {"cid": 222}]
    });
    let play = serde_json::json!({
        "durl": [{
            "url": "https://upos-sz-mirrorcos.bilivideo.com/page-2.mp4",
            "size": 12345
        }]
    });

    assert_eq!(cid_from_view(&view, Some(2)).unwrap(), 222);
    assert!(matches!(
        cid_from_view(&view, Some(3)),
        Err(Error::NotFound)
    ));

    let post = build_post_from_payloads_for_page(&view, &play, Some(2)).unwrap();
    assert_eq!(
        post.canonical_url.as_str(),
        "https://www.bilibili.com/video/BV1GJ411x7h7?p=2"
    );
    assert_eq!(post.download_file_stem(), "Bilibili_BV1GJ411x7h7_p2");
}

#[tokio::test]
async fn resolver_rejects_invalid_or_ambiguous_page_queries_without_network() {
    let resolver = BilibiliResolver::new().unwrap();
    for raw in [
        "https://www.bilibili.com/video/BV1GJ411x7h7?p=0",
        "https://www.bilibili.com/video/BV1GJ411x7h7?p=abc",
        "https://www.bilibili.com/video/BV1GJ411x7h7?p=1&p=2",
    ] {
        let url = url::Url::parse(raw).unwrap();
        assert!(matches!(page_from_url(&url), Err(Error::UnsupportedUrl)));
        assert!(matches!(
            resolver.resolve_text(raw).await,
            Err(Error::UnsupportedUrl)
        ));
    }
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
fn rejects_video_only_dash_tracks_as_playable_sources() {
    let play = serde_json::json!({
        "dash": {
            "video": [
                {
                    "id": 16,
                    "bandwidth": 500_000,
                    "width": 640,
                    "height": 360,
                    "codecs": "avc1.64001E",
                    "base_url": "https://upos-sz-mirrorcos.bilivideo.com/low.m4s"
                },
                {
                    "id": 80,
                    "bandwidth": 2_000_000,
                    "width": 1920,
                    "height": 1080,
                    "codecs": "hev1.1.6.L150.90",
                    "baseUrl": "https://upos-sz-mirrorhw.bilivideo.com/high.m4s",
                    "backup_url": ["https://upos-sz-mirrorali.bilivideo.com/high-b.m4s"]
                }
            ]
        }
    });
    assert!(collect_play_sources(&play).is_empty());
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

    let unrelated_akamai = serde_json::json!({
        "durl": [{"url": "https://unrelated.akamaized.net/video.mp4"}]
    });
    assert!(collect_play_sources(&unrelated_akamai).is_empty());

    let reviewed_akamai = serde_json::json!({
        "durl": [{"url": "https://upos-hz-mirrorakam.akamaized.net/video.mp4"}]
    });
    assert_eq!(collect_play_sources(&reviewed_akamai).len(), 1);
}

#[test]
fn credential_status_detects_sessdata() {
    let anon = BilibiliResolver::new().unwrap();
    assert_eq!(anon.credential_status(), CredentialStatus::Absent);
    assert!(!anon.is_authenticated());

    let ok = BilibiliResolver::with_cookie("SESSDATA=abc; bili_jct=x").unwrap();
    assert_eq!(ok.credential_status(), CredentialStatus::Present);
    assert!(ok.is_authenticated());

    let incomplete = BilibiliResolver::with_cookie("DedeUserID=1; foo=bar").unwrap();
    assert_eq!(incomplete.credential_status(), CredentialStatus::Incomplete);

    let empty = BilibiliResolver::with_cookie("SESSDATA=; bili_jct=x").unwrap();
    assert_eq!(empty.credential_status(), CredentialStatus::Incomplete);
}

#[test]
fn accepts_case_insensitive_scheme_and_host_but_not_malformed_bvids() {
    let url = extract_share_url("HTTPS://WWW.BILIBILI.COM/video/BV1GJ411x7h7").unwrap();
    assert_eq!(url.host_str(), Some("www.bilibili.com"));

    assert!(matches!(
        extract_share_url(&format!(
            "https://www.bilibili.com/video/BV{}",
            "a".repeat(65)
        )),
        Err(Error::UnsupportedUrl)
    ));
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
