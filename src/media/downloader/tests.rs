//! Tests for `MediaDownloader`.

use std::{
    collections::HashSet,
    fs::OpenOptions,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use reqwest::StatusCode;
use tokio::sync::mpsc;
use url::Url;
use uuid::Uuid;

use super::http::{check_response_status, parse_content_range};
use super::ssrf::{is_forbidden_ip, normalize_allowed_hosts, validate_media_url};
use super::write::{
    MIN_FREE_DISK_BYTES, PendingFile, disk_space_is_sufficient, effective_resume_offset,
    existing_complete_download, extension_from_content_type, extension_from_url,
    looks_like_media_header, media_task_path, path_with_better_extension, safe_file_stem,
    write_chunks,
};
use super::*;
use crate::platforms;

fn allowed_hosts() -> HashSet<String> {
    normalize_allowed_hosts(platforms::wechat::REVIEWED_MEDIA_HOSTS.iter().copied()).unwrap()
}

/// Ephemeral workspace under the system temp dir (never the repo root).
fn test_workspace() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("parse-kit-test-{}", Uuid::new_v4().hyphenated()))
}

#[test]
fn with_allowed_hosts_normalizes_and_exposes_the_set() {
    let downloader =
        MediaDownloader::with_allowed_hosts(test_workspace(), ["Example.COM", "cdn.example.org"])
            .unwrap();

    assert!(downloader.allowed_hosts().contains("example.com"));
    assert!(downloader.allowed_hosts().contains("cdn.example.org"));
    assert_eq!(downloader.allowed_hosts().len(), 2);
}

#[test]
fn with_allowed_hosts_rejects_empty_or_invalid_entries() {
    assert!(matches!(
        MediaDownloader::with_allowed_hosts(test_workspace(), std::iter::empty::<&str>()),
        Err(Error::Config(_))
    ));
    assert!(matches!(
        MediaDownloader::with_allowed_hosts(test_workspace(), [".bad", "ok.example"]),
        Err(Error::Config(_))
    ));
    assert!(matches!(
        MediaDownloader::with_allowed_hosts(test_workspace(), ["has space.example"]),
        Err(Error::Config(_))
    ));
    for invalid in [
        "-edge.example",
        "edge-.example",
        "a................................................................example",
    ] {
        assert!(matches!(
            MediaDownloader::with_allowed_hosts(test_workspace(), [invalid]),
            Err(Error::Config(_))
        ));
    }
}

#[test]
fn with_options_rejects_invalid_identity_headers() {
    let identity = DownloadRequestIdentity {
        user_agent: Some("valid\r\ninjected: value".to_owned()),
        ..DownloadRequestIdentity::default()
    };
    assert!(matches!(
        MediaDownloader::with_options(
            test_workspace(),
            ["media.example"],
            Duration::from_secs(1),
            identity,
        ),
        Err(Error::Config(_))
    ));
}

#[test]
fn for_wechat_uses_the_reviewed_host_set() {
    let downloader = MediaDownloader::for_wechat(test_workspace()).unwrap();
    for host in platforms::wechat::REVIEWED_MEDIA_HOSTS {
        assert!(
            downloader.allowed_hosts().contains(*host),
            "missing reviewed host {host}"
        );
    }
    assert_eq!(
        downloader.allowed_hosts().len(),
        platforms::wechat::REVIEWED_MEDIA_HOSTS.len()
    );
}

#[test]
fn host_allowlist_supports_suffix_rules() {
    let allowed = normalize_allowed_hosts([".douyinvod.com", "aweme.snssdk.com"]).unwrap();
    let ok = Url::parse("https://v3-web.douyinvod.com/path/video.mp4").unwrap();
    assert_eq!(
        validate_media_url(&ok, &allowed).unwrap(),
        "v3-web.douyinvod.com"
    );
    let exact = Url::parse("https://aweme.snssdk.com/aweme/v1/play/?video_id=x").unwrap();
    assert_eq!(
        validate_media_url(&exact, &allowed).unwrap(),
        "aweme.snssdk.com"
    );
    for raw in [
        "https://douyinvod.com/path",
        "https://evil-douyinvod.com/path",
        "https://example.com/path",
    ] {
        let url = Url::parse(raw).unwrap();
        assert!(validate_media_url(&url, &allowed).is_err(), "{raw}");
    }
}

#[test]
fn for_douyin_uses_reviewed_host_set() {
    let downloader = MediaDownloader::for_douyin(test_workspace()).unwrap();
    for host in platforms::douyin::REVIEWED_MEDIA_HOSTS {
        assert!(
            downloader.allowed_hosts().contains(*host),
            "missing reviewed host {host}"
        );
    }
}

#[test]
fn for_platform_maps_known_ids() {
    let wechat =
        MediaDownloader::for_platform(crate::PlatformId::Wechat, test_workspace()).unwrap();
    assert!(wechat.allowed_hosts().contains("finder.video.qq.com"));
    let douyin =
        MediaDownloader::for_platform(crate::PlatformId::Douyin, test_workspace()).unwrap();
    assert!(douyin.allowed_hosts().contains("aweme.snssdk.com"));
    let _ = MediaDownloader::for_platform(crate::PlatformId::Bilibili, test_workspace()).unwrap();
}

#[tokio::test]
async fn retries_only_transient_download_failures() {
    let transient_attempts = AtomicUsize::new(0);
    let value = retry_transient_downloads(
        || {
            let attempt = transient_attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                if attempt < 2 {
                    Err(Error::Network("temporary".into()))
                } else {
                    Ok(42_u8)
                }
            }
        },
        &[Duration::ZERO, Duration::ZERO],
    )
    .await
    .unwrap();
    assert_eq!(value, 42);
    assert_eq!(transient_attempts.load(Ordering::SeqCst), 3);

    let permanent_attempts = AtomicUsize::new(0);
    let error = retry_transient_downloads(
        || {
            permanent_attempts.fetch_add(1, Ordering::SeqCst);
            async { Err::<(), _>(Error::NotFound) }
        },
        &[Duration::ZERO, Duration::ZERO],
    )
    .await
    .unwrap_err();
    assert!(matches!(error, Error::NotFound));
    assert_eq!(permanent_attempts.load(Ordering::SeqCst), 1);

    let rate_limited_attempts = AtomicUsize::new(0);
    let error = retry_transient_downloads(
        || {
            rate_limited_attempts.fetch_add(1, Ordering::SeqCst);
            async { Err::<(), _>(Error::RateLimited) }
        },
        &[Duration::ZERO, Duration::ZERO],
    )
    .await
    .unwrap_err();
    assert!(matches!(error, Error::RateLimited));
    // Rate limits follow the same fixed schedule as transient network failures.
    assert_eq!(rate_limited_attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn cancelling_a_retry_backoff_returns_immediately() {
    let attempts = AtomicUsize::new(0);
    let retry_delays = [Duration::from_secs(60)];
    let retrying = retry_transient_downloads(
        || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Err::<(), _>(Error::Network("temporary".into())) }
        },
        &retry_delays,
    );

    assert!(
        tokio::time::timeout(Duration::from_millis(20), retrying)
            .await
            .is_err()
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[test]
fn classifies_only_temporary_http_statuses_for_retry() {
    assert!(matches!(
        check_response_status(StatusCode::INTERNAL_SERVER_ERROR),
        Err(Error::Network(_))
    ));
    assert!(matches!(
        check_response_status(StatusCode::REQUEST_TIMEOUT),
        Err(Error::Network(_))
    ));
    assert!(matches!(
        check_response_status(StatusCode::TOO_MANY_REQUESTS),
        Err(Error::RateLimited)
    ));
    assert!(matches!(
        check_response_status(StatusCode::NOT_FOUND),
        Err(Error::NotFound)
    ));
    assert!(matches!(
        check_response_status(StatusCode::BAD_REQUEST),
        Err(Error::Download(_))
    ));
}

#[test]
fn uses_a_canonical_hyphenated_uuid_for_temporary_video_names() {
    let path = media_task_path(
        test_workspace().as_path(),
        &Url::parse("https://cdn.example/v.mp4").unwrap(),
        None,
        0,
    );
    let file_name = path.file_name().unwrap().to_str().unwrap();
    let uuid = file_name.strip_suffix(".mp4").unwrap();

    assert_eq!(file_name.len(), 40);
    assert_eq!(uuid.as_bytes()[8], b'-');
    assert_eq!(uuid.as_bytes()[13], b'-');
    assert_eq!(uuid.as_bytes()[18], b'-');
    assert_eq!(uuid.as_bytes()[23], b'-');
    assert_eq!(
        Uuid::parse_str(uuid).unwrap().hyphenated().to_string(),
        uuid
    );
}

#[test]
fn media_task_path_uses_platform_stem_and_sequence() {
    let dir = test_workspace();
    let url = Url::parse("https://cdn.example/v.mp4").unwrap();
    let stem = "Wechat_AzJ7CGPYWD";

    assert_eq!(
        media_task_path(dir.as_path(), &url, Some(stem), 0)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap(),
        "Wechat_AzJ7CGPYWD.mp4"
    );
    assert_eq!(
        media_task_path(dir.as_path(), &url, Some(stem), 1)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap(),
        "Wechat_AzJ7CGPYWD_1.mp4"
    );
}

#[test]
fn media_task_path_confines_untrusted_stems_to_the_workspace() {
    let dir = test_workspace();
    let url = Url::parse("https://cdn.example/v.mp4").unwrap();
    let path = media_task_path(dir.as_path(), &url, Some("../../outside/video"), 0);

    assert_eq!(path.parent(), Some(dir.as_path()));
    assert_eq!(path.file_name().unwrap(), "outside_video.mp4");
    assert_eq!(safe_file_stem("___"), None);
}

#[test]
fn with_file_stem_is_exposed_on_downloader() {
    let downloader = MediaDownloader::for_wechat(test_workspace())
        .unwrap()
        .with_file_stem("Wechat_AzJ7CGPYWD");
    assert_eq!(downloader.file_stem(), Some("Wechat_AzJ7CGPYWD"));
}

#[test]
fn with_file_stem_sanitizes_path_components() {
    let downloader = MediaDownloader::for_wechat(test_workspace())
        .unwrap()
        .with_file_stem("../Wechat Demo/clip");
    assert_eq!(downloader.file_stem(), Some("Wechat_Demo_clip"));
}

#[test]
fn accepts_only_reviewed_https_media_urls() {
    let allowed = allowed_hosts();
    let valid = Url::parse("https://finder.video.qq.com/path?token=secret").unwrap();
    assert_eq!(
        validate_media_url(&valid, &allowed).unwrap(),
        "finder.video.qq.com"
    );

    for raw in [
        "http://finder.video.qq.com/path",
        "https://user:pass@finder.video.qq.com/path",
        "https://finder.video.qq.com/path#fragment",
        "https://finder.video.qq.com.evil.test/path",
        "https://finder.video.qq.com./path",
        "https://qq.com/path",
        "https://127.0.0.1/path",
        "https://finder.video.qq.com:0/path",
    ] {
        let url = Url::parse(raw).unwrap();
        assert!(validate_media_url(&url, &allowed).is_err(), "{raw}");
    }

    // CDN edges may serve media on non-443 HTTPS ports.
    let non_default_port =
        Url::parse("https://finder.video.qq.com:20443/path?token=secret").unwrap();
    assert_eq!(
        validate_media_url(&non_default_port, &allowed).unwrap(),
        "finder.video.qq.com"
    );
}

#[tokio::test]
async fn direct_url_download_still_rejects_disallowed_hosts_before_network_io() {
    let downloader = MediaDownloader::with_options(
        test_workspace(),
        HashSet::from(["allowed.example".to_owned()]),
        Duration::from_secs(17),
        DownloadRequestIdentity::default(),
    )
    .unwrap();
    let disallowed = Url::parse("https://127.0.0.1/cover.jpg").unwrap();

    let error = downloader.download_url(&disallowed).await.unwrap_err();
    assert!(matches!(error, Error::Download(_)));
}

#[test]
fn classifies_non_public_ipv4_addresses() {
    for raw in [
        "0.0.0.0",
        "10.1.2.3",
        "100.64.0.1",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.1",
        "192.168.1.1",
        "198.18.0.1",
        "224.0.0.1",
    ] {
        assert!(is_forbidden_ip(raw.parse().unwrap()), "{raw}");
    }
    assert!(!is_forbidden_ip("1.1.1.1".parse().unwrap()));
}

#[test]
fn classifies_non_public_ipv6_addresses() {
    for raw in [
        "::",
        "::1",
        "::127.0.0.1",
        "64:ff9b:1::1",
        "100::1",
        "2001:2::1",
        "2001:20::1",
        "3fff::1",
        "5f00::1",
        "fc00::1",
        "fe80::1",
        "ff02::1",
        "2001:db8::1",
    ] {
        assert!(is_forbidden_ip(raw.parse().unwrap()), "{raw}");
    }
    assert!(!is_forbidden_ip("2606:4700:4700::1111".parse().unwrap()));
    assert!(is_forbidden_ip("::ffff:127.0.0.1".parse().unwrap()));
}

#[cfg(unix)]
#[test]
fn preserves_disk_headroom_before_accepting_a_write() {
    let reserve = u128::from(MIN_FREE_DISK_BYTES);

    assert!(!disk_space_is_sufficient(reserve - 1, 0));
    assert!(disk_space_is_sufficient(reserve, 0));
    assert!(!disk_space_is_sufficient(reserve + 9, 10));
    assert!(disk_space_is_sufficient(reserve + 10, 10));
}

#[tokio::test]
async fn cleanup_is_idempotent() {
    let directory = std::env::temp_dir().join(format!(
        "parse-kit-downloader-test-{}",
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("media");
    std::fs::write(&path, b"test").unwrap();
    let media = DownloadedMedia::new(path.clone(), 4);

    media.cleanup().await.unwrap();
    media.cleanup().await.unwrap();
    assert!(!path.exists());
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn reports_each_crossed_threshold_once_after_writes() {
    let directory = std::env::temp_dir().join(format!(
        "parse-kit-progress-test-{}",
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("media");
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let pending_file = PendingFile::new(path.clone(), file);

    let events = Arc::new(Mutex::new(Vec::new()));
    let callback_events = Arc::clone(&events);
    let callback: ProgressCallback = Arc::new(move |progress| {
        callback_events.lock().unwrap().push(progress);
    });
    let (reporter, guard) = ProgressReporter::new(Some(8), Some(callback), 0);
    let guard = guard.unwrap();

    let (sender, mut receiver) = mpsc::channel(4);
    sender.try_send(vec![1, 2]).unwrap();
    sender.try_send(vec![3, 4, 5, 6]).unwrap();
    sender.try_send(vec![7, 8]).unwrap();
    drop(sender);

    let disk_write_budget = Arc::new(StdMutex::new(DiskWriteBudget::default()));
    let mut outcome = write_chunks(
        pending_file,
        &mut receiver,
        reporter,
        disk_write_budget,
        None,
        0,
    )
    .unwrap();
    assert_eq!(
        std::fs::read(&outcome.media.path).unwrap(),
        (1_u8..=8).collect::<Vec<_>>()
    );
    // 2/8 → 25%, 6/8 → 75%, 8/8 intermediate still max 99 until complete.
    let percents: Vec<u8> = events.lock().unwrap().iter().map(|e| e.percent).collect();
    assert!(percents.contains(&20));
    assert!(percents.contains(&25));
    assert!(percents.contains(&75));
    assert_eq!(*percents.last().unwrap(), 99);
    assert!(!percents.contains(&100));

    outcome
        .progress_reporter
        .as_mut()
        .unwrap()
        .report_complete(outcome.media.bytes);
    let percents: Vec<u8> = events.lock().unwrap().iter().map(|e| e.percent).collect();
    assert_eq!(*percents.last().unwrap(), 100);
    assert!(percents.contains(&100));
    let event_count = percents.len();
    outcome
        .progress_reporter
        .as_mut()
        .unwrap()
        .report_complete(outcome.media.bytes);
    assert_eq!(events.lock().unwrap().len(), event_count);

    drop(guard);
    drop(outcome);
    assert!(!path.exists());
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn does_not_report_percent_without_a_trusted_total() {
    let callback: ProgressCallback = Arc::new(|_| panic!("unexpected progress event"));
    let (reporter, guard) = ProgressReporter::new(None, Some(callback), 0);
    assert!(reporter.is_none());
    assert!(guard.is_none());
}

#[test]
fn progress_guard_suppresses_events_after_cancellation() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let callback_events = Arc::clone(&events);
    let callback: ProgressCallback = Arc::new(move |progress| {
        callback_events.lock().unwrap().push(progress);
    });
    let (mut reporter, guard) = ProgressReporter::new(Some(100), Some(callback), 0);
    drop(guard);

    reporter.as_mut().unwrap().report_intermediate(75);
    reporter.as_mut().unwrap().report_complete(100);

    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn resumed_progress_does_not_replay_completed_thresholds() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let callback_events = Arc::clone(&events);
    let callback: ProgressCallback = Arc::new(move |progress| {
        callback_events.lock().unwrap().push(progress.percent);
    });
    let (mut reporter, _guard) = ProgressReporter::new(Some(100), Some(callback), 60);

    reporter.as_mut().unwrap().report_intermediate(65);

    assert_eq!(events.lock().unwrap().as_slice(), &[61, 62, 63, 64, 65]);
}

#[test]
fn effective_resume_offset_handles_partial_and_full_ok() {
    use reqwest::StatusCode;
    assert_eq!(effective_resume_offset(0, StatusCode::OK), Some(0));
    assert_eq!(
        effective_resume_offset(100, StatusCode::PARTIAL_CONTENT),
        Some(100)
    );
    assert_eq!(effective_resume_offset(100, StatusCode::OK), Some(0));
    assert_eq!(effective_resume_offset(100, StatusCode::NOT_FOUND), None);
}

#[test]
fn parses_and_validates_content_ranges() {
    use reqwest::header::{CONTENT_RANGE, HeaderMap, HeaderValue};

    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_RANGE,
        HeaderValue::from_static("bytes 100-199/1000"),
    );
    let range = parse_content_range(&headers).unwrap();
    assert_eq!(range.start, Some(100));
    assert_eq!(range.end, Some(199));
    assert_eq!(range.total, Some(1000));
    assert_eq!(range.response_length(), Some(100));

    headers.insert(CONTENT_RANGE, HeaderValue::from_static("bytes */1000"));
    let range = parse_content_range(&headers).unwrap();
    assert_eq!(
        (range.start, range.end, range.total),
        (None, None, Some(1000))
    );

    for invalid in ["items 0-1/2", "bytes 2-1/3", "bytes 0-3/3"] {
        headers.insert(
            CONTENT_RANGE,
            HeaderValue::from_bytes(invalid.as_bytes()).unwrap(),
        );
        assert!(parse_content_range(&headers).is_none(), "{invalid}");
    }
}

#[test]
fn extension_from_url_guesses_common_types() {
    assert_eq!(
        extension_from_url(&Url::parse("https://x/a/b.mp4?x=1").unwrap()),
        "mp4"
    );
    assert_eq!(
        extension_from_url(&Url::parse("https://p3.douyinpic.com/img.jpeg").unwrap()),
        "jpg"
    );
    assert_eq!(
        extension_from_url(&Url::parse("https://x/play/?video_id=1").unwrap()),
        "mp4"
    );
    // WeChat Channels CDN: extension-less stodownload paths.
    assert_eq!(
        extension_from_url(
            &Url::parse("https://finder.video.qq.com/251/20302/stodownload?encfilekey=k&token=t")
                .unwrap()
        ),
        "mp4"
    );
    assert_eq!(
        extension_from_url(
            &Url::parse(
                "https://finder.video.qq.com/251/20304/stodownload?encfilekey=k&token=t&picformat=200&wxampicformat=503"
            )
            .unwrap()
        ),
        "jpg"
    );
}

#[test]
fn extension_from_content_type_maps_common_mimes() {
    assert_eq!(extension_from_content_type("video/mp4"), Some("mp4"));
    assert_eq!(
        extension_from_content_type("video/mp4; charset=binary"),
        Some("mp4")
    );
    assert_eq!(extension_from_content_type("image/jpeg"), Some("jpg"));
    assert_eq!(extension_from_content_type("video/mpeg"), Some("mpg"));
    assert_eq!(extension_from_content_type("video/mp2t"), Some("ts"));
    assert_eq!(extension_from_content_type("audio/aac"), Some("aac"));
    assert_eq!(extension_from_content_type("image/avif"), Some("avif"));
    assert_eq!(
        extension_from_content_type("application/octet-stream"),
        None
    );
    assert_eq!(extension_from_content_type("image/svg+xml"), None);
}

#[test]
fn transport_stream_signature_requires_multiple_packets() {
    let mut transport_stream = vec![0_u8; 377];
    transport_stream[0] = 0x47;
    transport_stream[188] = 0x47;
    transport_stream[376] = 0x47;

    assert!(looks_like_media_header(&transport_stream));
    assert!(!looks_like_media_header(&[0x47; 16]));
}

#[test]
fn mpeg_audio_signature_rejects_reserved_header_fields() {
    assert!(looks_like_media_header(&[0xff, 0xfb, 0x90, 0x64]));
    assert!(!looks_like_media_header(&[0xff, 0xe8, 0x90, 0x64]));
    assert!(!looks_like_media_header(&[0xff, 0xfb, 0xf0, 0x64]));
}

#[test]
fn recognizes_common_stream_container_signatures() {
    assert!(looks_like_media_header(b"OggS\0\x02payload"));
    assert!(looks_like_media_header(&[0, 0, 1, 0xba, 0, 0, 0, 0]));
    assert!(looks_like_media_header(&[
        0xff, 0xf1, 0x50, 0x80, 0, 0x1f, 0xfc,
    ]));
}

#[test]
fn path_with_better_extension_only_upgrades_bin() {
    let dir = test_workspace();
    let bin = dir.join("clip.bin");
    let mp4 = dir.join("clip.mp4");
    assert_eq!(
        path_with_better_extension(bin.clone(), "mp4"),
        dir.join("clip.mp4")
    );
    assert_eq!(path_with_better_extension(mp4.clone(), "jpg"), mp4);
    assert_eq!(path_with_better_extension(bin, "bin"), dir.join("clip.bin"));
}

#[tokio::test]
async fn existing_complete_download_reuses_bmff_file() {
    let dir = test_workspace();
    std::fs::create_dir_all(&dir).unwrap();
    // Minimal ftyp box (size 16, type ftyp) + padding so length >= 1024.
    let mut bytes = vec![
        0, 0, 0, 16, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm', 0, 0, 0, 0,
    ];
    bytes.resize(2048, 0);
    let path = dir.join("wechat_demo.mp4");
    std::fs::write(&path, &bytes).unwrap();

    let found = existing_complete_download(dir.as_path(), Some("wechat_demo"), 0, "mp4", None)
        .await
        .expect("should reuse local file");
    assert_eq!(found.0, path);
    assert_eq!(found.1, 2048);

    // Smaller than size_hint → treat as incomplete.
    assert!(
        existing_complete_download(dir.as_path(), Some("wechat_demo"), 0, "mp4", Some(4096))
            .await
            .is_none()
    );

    // Matching length alone must not make an unrelated/corrupt file reusable.
    let invalid = dir.join("invalid.mp4");
    std::fs::write(&invalid, vec![b'x'; 2048]).unwrap();
    assert!(
        existing_complete_download(dir.as_path(), Some("invalid"), 0, "mp4", Some(2048))
            .await
            .is_none()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[tokio::test]
async fn existing_complete_download_never_reuses_a_symlink() {
    let dir = test_workspace();
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("outside.mp4");
    std::fs::write(&target, vec![0_u8; 2048]).unwrap();
    let link = dir.join("wechat_demo.mp4");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    assert!(
        existing_complete_download(dir.as_path(), Some("wechat_demo"), 0, "mp4", Some(1024))
            .await
            .is_none()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn into_path_disarms_drop_cleanup() {
    let dir = std::env::temp_dir().join(format!("pk-keep-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("keep.bin");
    std::fs::write(&path, b"hi").unwrap();
    let media = DownloadedMedia::new(path.clone(), 2);
    let kept = media.into_path();
    assert_eq!(kept, path);
    assert!(path.exists());
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&dir).unwrap();
}

#[test]
fn with_timeout_tightens_request_timeout() {
    let downloader = MediaDownloader::with_options(
        test_workspace(),
        HashSet::from(["example.com".to_owned()]),
        Duration::from_secs(120),
        DownloadRequestIdentity::default(),
    )
    .unwrap();
    let tighter = downloader.with_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(tighter.request_timeout, Duration::from_secs(30));
    assert!(downloader.with_timeout(Duration::ZERO).is_err());
}
