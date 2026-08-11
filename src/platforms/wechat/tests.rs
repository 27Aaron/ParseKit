use super::api::{ParseData, cookie_value};
use super::parse::{build_post, has_matching_media_identity, merge_source_metadata};
use super::share::{
    NormalizedShareUrl, derive_direct_media_url, extract_share_url, normalize_share_url,
};
use crate::{
    Error,
    model::{MediaSource, MediaSourceKind, VideoCodec},
};
use ::url::Url;

use super::hosts::REVIEWED_MEDIA_HOSTS;

#[test]
fn extracts_and_canonicalizes_share_url_from_text() {
    let url = extract_share_url("看看这个 https://weixin.qq.com/sph/AzJ7CGPYWD。").unwrap();
    assert_eq!(url.as_str(), "https://weixin.qq.com/sph/AzJ7CGPYWD");
}

#[test]
fn rejects_preview_and_other_link_forms_during_extraction() {
    for input in [
        "https://channels.weixin.qq.com/finder-preview/pages/sph?id=AzJ7CGPYWD",
        "http://weixin.qq.com/sph/AzJ7CGPYWD",
        "https://WEIXIN.QQ.COM/sph/AzJ7CGPYWD",
        "https://weixin.qq.com/other/AzJ7CGPYWD",
    ] {
        assert!(matches!(
            extract_share_url(input),
            Err(Error::UnsupportedUrl)
        ));
    }
}

#[test]
fn rejects_spoofed_or_insecure_urls() {
    for raw in [
        "http://weixin.qq.com/sph/AzJ7CGPYWD",
        "https://weixin.qq.com.evil.test/sph/AzJ7CGPYWD",
        "https://weixin.qq.com./sph/AzJ7CGPYWD",
        "https://user@weixin.qq.com/sph/AzJ7CGPYWD",
        "https://weixin.qq.com/other/AzJ7CGPYWD",
        "https://weixin.qq.com/sph/AzJ7CGPYWD/",
        "https://weixin.qq.com/sph/AzJ7CGPYWD?from=share",
        "https://weixin.qq.com/sph/AzJ7CGPYWD#fragment",
    ] {
        assert!(
            normalize_share_url(&Url::parse(raw).unwrap()).is_err(),
            "{raw}"
        );
    }
}

#[test]
fn derives_direct_media_url_structurally() {
    let source = Url::parse(
        "https://finder.video.qq.com/path/video.mp4?token=t%2Bv&quality=hd&encfilekey=e%26k",
    )
    .unwrap();
    let direct = derive_direct_media_url(&source).unwrap();
    assert_eq!(
        direct.as_str(),
        "https://finder.video.qq.com/path/video.mp4?encfilekey=e%26k&token=t%2Bv"
    );
}

#[test]
fn derives_direct_media_urls_for_every_reviewed_host() {
    for host in REVIEWED_MEDIA_HOSTS {
        let source = Url::parse(&format!(
            "https://{host}/video.mp4?token=token&quality=hd&encfilekey=key"
        ))
        .unwrap();
        let direct = derive_direct_media_url(&source).unwrap();
        assert_eq!(direct.host_str(), Some(*host));
        assert_eq!(direct.query(), Some("encfilekey=key&token=token"));
    }
}

#[test]
fn normalizes_clean_direct_url_and_rejects_incomplete_urls() {
    let already_clean =
        Url::parse("https://finder.video.qq.com/video.mp4?encfilekey=key&token=token").unwrap();
    let missing = Url::parse("https://finder.video.qq.com/video.mp4?token=token").unwrap();
    let reverse_order =
        Url::parse("https://finder.video.qq.com/video.mp4?token=token&encfilekey=key").unwrap();
    assert_eq!(
        derive_direct_media_url(&already_clean).unwrap(),
        already_clean
    );
    assert!(derive_direct_media_url(&missing).is_none());
    assert_eq!(
        derive_direct_media_url(&reverse_order).unwrap().query(),
        Some("encfilekey=key&token=token")
    );
    let dotted =
        Url::parse("https://finder.video.qq.com./video.mp4?encfilekey=key&token=token").unwrap();
    assert!(derive_direct_media_url(&dotted).is_none());

    let ambiguous =
        Url::parse("https://finder.video.qq.com/video.mp4?encfilekey=key&token=one&token=two")
            .unwrap();
    assert!(derive_direct_media_url(&ambiguous).is_none());
}

#[test]
fn matches_media_identity_only_when_path_and_signed_values_are_compatible() {
    let direct = Url::parse("https://finder.video.qq.com/shared.mp4?token=shared-token").unwrap();
    let candidate =
        Url::parse("https://finder.video.qq.com/shared.mp4?token=shared-token&encfilekey=key")
            .unwrap();
    assert!(has_matching_media_identity(&direct, &candidate));

    let conflicting_token =
        Url::parse("https://finder.video.qq.com/shared.mp4?token=other-token").unwrap();
    let conflicting_key = Url::parse(
        "https://finder.video.qq.com/shared.mp4?token=shared-token&encfilekey=other-key",
    )
    .unwrap();
    let keyed_direct =
        Url::parse("https://finder.video.qq.com/shared.mp4?token=shared-token&encfilekey=key")
            .unwrap();
    let other_path =
        Url::parse("https://finder.video.qq.com/other.mp4?token=shared-token").unwrap();
    assert!(!has_matching_media_identity(&direct, &conflicting_token));
    assert!(!has_matching_media_identity(
        &keyed_direct,
        &conflicting_key
    ));
    assert!(!has_matching_media_identity(&direct, &other_path));
}

#[test]
fn parses_cookie_without_logging_or_decoding_it() {
    let cookie = "a=1; hy_user=user-id; token=a=b=c";
    assert_eq!(cookie_value(cookie, "hy_user").as_deref(), Some("user-id"));
    assert_eq!(cookie_value(cookie, "token").as_deref(), Some("a=b=c"));
}

#[test]
fn assess_yuanbao_cookie_detects_markers() {
    use crate::{CredentialStatus, platforms::wechat::assess_yuanbao_cookie};

    assert_eq!(assess_yuanbao_cookie(""), CredentialStatus::Absent);
    assert_eq!(
        assess_yuanbao_cookie("foo=bar; baz=1"),
        CredentialStatus::Incomplete
    );
    assert_eq!(
        assess_yuanbao_cookie("hy_user=; hy_token="),
        CredentialStatus::Incomplete
    );
    assert_eq!(
        assess_yuanbao_cookie("hy_user=u1"),
        CredentialStatus::Incomplete
    );
    assert_eq!(
        assess_yuanbao_cookie("hy_token=t1"),
        CredentialStatus::Incomplete
    );
    assert_eq!(
        assess_yuanbao_cookie("hy_user=u1; hy_token=t1"),
        CredentialStatus::Present
    );
    let resolver =
        super::WechatResolver::new("hy_user=u; token=t").expect("cookie with markers should build");
    assert_eq!(resolver.credential_status(), CredentialStatus::Present);
}

#[test]
fn builds_post_from_feed_fixture_and_derives_media_from_preferred_h264_seed() {
    let normalized = NormalizedShareUrl {
        share_id: "AzJ7CGPYWD".to_owned(),
        canonical_url: Url::parse("https://weixin.qq.com/sph/AzJ7CGPYWD").unwrap(),
    };
    let parse_data = ParseData {
        wx_export_id: "fallback-export-id".to_owned(),
        cover_url: String::new(),
        desc: "备用描述".to_owned(),
        playable_url:
            "https://channels.weixin.qq.com/finder-preview/pages/feed?token=dummy&eid=export-id"
                .to_owned(),
    };
    let feed: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/wechat/feed_h264_preferred.json"
    ))
    .expect("committed wechat feed fixture");

    let post = build_post(normalized, parse_data, feed, "export-id".to_owned()).unwrap();
    assert_eq!(post.post_id, "export-id");
    assert_eq!(post.title.as_deref(), Some("测试视频"));
    assert_eq!(
        post.primary_video().unwrap().provenance,
        MediaSourceKind::Derived
    );
    assert_eq!(post.primary_video().unwrap().codec, VideoCodec::H264);
    assert_eq!(
        post.primary_video().unwrap().url.query(),
        Some("encfilekey=h&token=t")
    );
    assert_eq!(post.primary_video().unwrap().size_hint, Some(123456));
    assert_eq!(
        post.video_fallbacks()
            .iter()
            .map(|source| source.codec)
            .collect::<Vec<_>>(),
        [VideoCodec::H265, VideoCodec::Unknown]
    );
}

#[test]
fn accepts_root_level_feed_shape_and_prefers_direct_source() {
    let normalized = NormalizedShareUrl {
        share_id: "AzJ7CGPYWD".to_owned(),
        canonical_url: Url::parse("https://weixin.qq.com/sph/AzJ7CGPYWD").unwrap(),
    };
    let parse_data = ParseData {
        wx_export_id: String::new(),
        cover_url: String::new(),
        desc: String::new(),
        playable_url: "https://example.invalid/?token=dummy".to_owned(),
    };
    let feed = serde_json::json!({
        "feedInfo": {
            "h264VideoInfo": {
                "videoUrl": "https://finder.video.qq.com/candidate.mp4?encfilekey=k&token=t&quality=hd"
            },
            "originVideoUrl": "https://finder.video.qq.com/direct.mp4?token=t"
        }
    });

    let post = build_post(normalized, parse_data, feed, "export-id".to_owned()).unwrap();
    assert_eq!(
        post.primary_video().unwrap().provenance,
        MediaSourceKind::Direct
    );
    assert_eq!(
        post.primary_video().unwrap().url.as_str(),
        "https://finder.video.qq.com/direct.mp4?token=t"
    );
    assert_eq!(post.video_fallbacks().len(), 1);
    assert_eq!(post.video_fallbacks()[0].codec, VideoCodec::H264);
}

#[test]
fn preserves_direct_identity_and_merges_metadata_when_urls_are_equal() {
    let normalized = NormalizedShareUrl {
        share_id: "AzJ7CGPYWD".to_owned(),
        canonical_url: Url::parse("https://weixin.qq.com/sph/AzJ7CGPYWD").unwrap(),
    };
    let parse_data = ParseData {
        wx_export_id: String::new(),
        cover_url: String::new(),
        desc: String::new(),
        playable_url: "https://example.invalid/?token=dummy".to_owned(),
    };
    let shared_url = "https://findermp.video.qq.com/shared.mp4?token=t";
    let feed = serde_json::json!({
        "feedInfo": {
            "h264VideoInfo": {
                "videoUrl": shared_url,
                "width": 1080,
                "height": 1920,
                "fileSize": 7654321,
                "decodeKey": "2136343393"
            },
            "originVideoUrl": shared_url
        }
    });

    let post = build_post(normalized, parse_data, feed, "export-id".to_owned()).unwrap();
    assert_eq!(
        post.primary_video().unwrap().provenance,
        MediaSourceKind::Direct
    );
    assert_eq!(post.primary_video().unwrap().codec, VideoCodec::H264);
    assert_eq!(
        (
            post.primary_video().unwrap().width,
            post.primary_video().unwrap().height
        ),
        (Some(1080), Some(1920))
    );
    assert_eq!(post.primary_video().unwrap().size_hint, Some(7_654_321));
    assert_eq!(
        post.primary_video().unwrap().decode_key,
        Some(2_136_343_393)
    );
}

#[test]
fn direct_source_inherits_safe_matching_candidate_metadata() {
    let normalized = NormalizedShareUrl {
        share_id: "AzJ7CGPYWD".to_owned(),
        canonical_url: Url::parse("https://weixin.qq.com/sph/AzJ7CGPYWD").unwrap(),
    };
    let parse_data = ParseData {
        wx_export_id: String::new(),
        cover_url: String::new(),
        desc: String::new(),
        playable_url: "https://example.invalid/?token=dummy".to_owned(),
    };
    let feed = serde_json::json!({
        "feedInfo": {
            "h265VideoInfo": {
                "videoUrl": "https://finder.video.wechat.com/shared.mp4?quality=hd&token=t&encfilekey=k",
                "width": 1920,
                "height": 1080,
                "fileSize": 123456,
                "decodeKey": 987654321
            },
            "originVideoUrl": "https://finder.video.wechat.com/shared.mp4?token=t"
        }
    });

    let post = build_post(normalized, parse_data, feed, "export-id".to_owned()).unwrap();
    assert_eq!(
        post.primary_video().unwrap().provenance,
        MediaSourceKind::Direct
    );
    assert_eq!(post.primary_video().unwrap().codec, VideoCodec::H265);
    assert_eq!(
        (
            post.primary_video().unwrap().width,
            post.primary_video().unwrap().height
        ),
        (Some(1920), Some(1080))
    );
    assert_eq!(post.primary_video().unwrap().decode_key, Some(987_654_321));
    assert_eq!(post.primary_video().unwrap().size_hint, None);
}

#[test]
fn merge_does_not_overwrite_existing_decode_key() {
    let mut target = MediaSource {
        url: Url::parse("https://finder.video.qq.com/a.mp4?token=t").unwrap(),
        codec: VideoCodec::Unknown,
        provenance: MediaSourceKind::Direct,
        width: None,
        height: None,
        size_hint: None,
        decode_key: Some(111),
        label: None,
        bitrate_bps: None,
    };
    let source = MediaSource {
        url: target.url.clone(),
        codec: VideoCodec::H264,
        provenance: MediaSourceKind::H264,
        width: Some(1),
        height: Some(1),
        size_hint: Some(9),
        decode_key: Some(222),
        label: None,
        bitrate_bps: None,
    };
    merge_source_metadata(&mut target, &source, true, true);
    assert_eq!(target.decode_key, Some(111));
    assert_eq!(target.codec, VideoCodec::H264);
}

#[test]
fn direct_source_does_not_inherit_an_ambiguous_decode_key() {
    let normalized = NormalizedShareUrl {
        share_id: "AzJ7CGPYWD".to_owned(),
        canonical_url: Url::parse("https://weixin.qq.com/sph/AzJ7CGPYWD").unwrap(),
    };
    let parse_data = ParseData {
        wx_export_id: String::new(),
        cover_url: String::new(),
        desc: String::new(),
        playable_url: "https://example.invalid/?token=dummy".to_owned(),
    };
    let feed = serde_json::json!({
        "feedInfo": {
            "h264VideoInfo": {
                "videoUrl": "https://finder.video.qq.com/shared.mp4?encfilekey=h264-key&token=shared-token",
                "decodeKey": 111
            },
            "h265VideoInfo": {
                "videoUrl": "https://finder.video.qq.com/shared.mp4?encfilekey=h265-key&token=shared-token",
                "decodeKey": 222
            },
            "originVideoUrl": "https://finder.video.qq.com/shared.mp4?token=shared-token"
        }
    });

    let post = build_post(normalized, parse_data, feed, "export-id".to_owned()).unwrap();
    assert_eq!(
        post.primary_video().unwrap().provenance,
        MediaSourceKind::Direct
    );
    assert_eq!(post.primary_video().unwrap().codec, VideoCodec::Unknown);
    assert_eq!(post.primary_video().unwrap().decode_key, None);
}

#[test]
fn same_url_candidates_keep_distinct_decode_keys_as_fallbacks() {
    let normalized = NormalizedShareUrl {
        share_id: "AzJ7CGPYWD".to_owned(),
        canonical_url: Url::parse("https://weixin.qq.com/sph/AzJ7CGPYWD").unwrap(),
    };
    let parse_data = ParseData {
        wx_export_id: String::new(),
        cover_url: String::new(),
        desc: String::new(),
        playable_url: "https://example.invalid/?token=dummy".to_owned(),
    };
    let feed = serde_json::json!({
        "feedInfo": {
            "h264VideoInfo": {
                "videoUrl": "https://finder.video.qq.com/shared.mp4?encfilekey=k&token=t&quality=hd",
                "decodeKey": 111
            },
            "h265VideoInfo": {
                "videoUrl": "https://finder.video.qq.com/shared.mp4?token=t&encfilekey=k&quality=sd",
                "decodeKey": 222
            },
            "originVideoUrl": "https://finder.video.qq.com/shared.mp4?encfilekey=k&token=t"
        }
    });

    let post = build_post(normalized, parse_data, feed, "export-id".to_owned()).unwrap();
    assert_eq!(post.primary_video().unwrap().decode_key, None);
    assert_eq!(
        post.video_fallbacks()
            .iter()
            .map(|source| (source.codec, source.decode_key))
            .collect::<Vec<_>>(),
        [(VideoCodec::H264, Some(111)), (VideoCodec::H265, Some(222))]
    );
    assert!(
        post.video_fallbacks()
            .iter()
            .all(|source| source.url == post.primary_video().unwrap().url)
    );
}

#[test]
fn prefers_higher_resolution_and_size_among_fallbacks() {
    let normalized = NormalizedShareUrl {
        share_id: "AzJ7CGPYWD".to_owned(),
        canonical_url: Url::parse("https://weixin.qq.com/sph/AzJ7CGPYWD").unwrap(),
    };
    let parse_data = ParseData {
        wx_export_id: String::new(),
        cover_url: String::new(),
        desc: String::new(),
        playable_url: "https://example.invalid/?token=dummy".to_owned(),
    };
    let feed = serde_json::json!({
        "feedInfo": {
            "h264VideoInfo": {
                "videoUrl": "https://finder.video.qq.com/small.mp4?encfilekey=s&token=t",
                "width": 720,
                "height": 1280,
                "fileSize": 1_000_000
            },
            "h265VideoInfo": {
                "videoUrl": "https://finder.video.qq.com/large.mp4?encfilekey=l&token=t",
                "width": 1080,
                "height": 1920,
                "fileSize": 5_000_000
            }
        }
    });

    let post = build_post(normalized, parse_data, feed, "export-id".to_owned()).unwrap();
    assert_eq!(post.primary_video().unwrap().codec, VideoCodec::H265);
    assert_eq!(post.primary_video().unwrap().width, Some(1080));
    assert_eq!(post.video_fallbacks()[0].codec, VideoCodec::H264);
}

#[test]
fn rejects_a_post_when_no_media_source_can_be_derived() {
    let normalized = NormalizedShareUrl {
        share_id: "AzJ7CGPYWD".to_owned(),
        canonical_url: Url::parse("https://weixin.qq.com/sph/AzJ7CGPYWD").unwrap(),
    };
    let parse_data = ParseData {
        wx_export_id: String::new(),
        cover_url: String::new(),
        desc: String::new(),
        playable_url: "https://example.invalid/?token=dummy".to_owned(),
    };
    let feed = serde_json::json!({
        "feedInfo": {
            "videoUrl": "https://finder.video.qq.com/fallback.mp4?token=t",
            "h265VideoInfo": {
                "videoUrl": "https://finder.video.qq.com/h265.mp4?token=t"
            }
        }
    });

    let error = build_post(normalized, parse_data, feed, "export-id".to_owned()).unwrap_err();
    assert!(matches!(error, Error::MediaUnavailable));
}

#[test]
fn tries_h265_when_h264_cannot_derive_a_media_source() {
    let normalized = NormalizedShareUrl {
        share_id: "AzJ7CGPYWD".to_owned(),
        canonical_url: Url::parse("https://weixin.qq.com/sph/AzJ7CGPYWD").unwrap(),
    };
    let parse_data = ParseData {
        wx_export_id: String::new(),
        cover_url: String::new(),
        desc: String::new(),
        playable_url: "https://example.invalid/?token=dummy".to_owned(),
    };
    let feed = serde_json::json!({
        "feedInfo": {
            "h264VideoInfo": {
                "videoUrl": "https://finder.video.qq.com/h264.mp4?token=t"
            },
            "h265VideoInfo": {
                "videoUrl": "https://finder.video.qq.com/h265.mp4?encfilekey=k&token=t&quality=hd"
            },
            "videoUrl": "https://finder.video.qq.com/fallback.mp4?token=t"
        }
    });

    let post = build_post(normalized, parse_data, feed, "export-id".to_owned()).unwrap();
    assert_eq!(
        post.primary_video().unwrap().provenance,
        MediaSourceKind::Derived
    );
    assert_eq!(post.primary_video().unwrap().codec, VideoCodec::H265);
    assert_eq!(
        post.primary_video().unwrap().url.as_str(),
        "https://finder.video.qq.com/h265.mp4?encfilekey=k&token=t"
    );
}
