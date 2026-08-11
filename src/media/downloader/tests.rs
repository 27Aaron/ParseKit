//! Tests for `MediaDownloader`.

use std::{
    collections::HashSet,
    fs::OpenOptions,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use reqwest::StatusCode;
use tokio::sync::mpsc;
use url::Url;
use uuid::Uuid;

use super::http::check_response_status;
use super::ssrf::{is_forbidden_ip, normalize_allowed_hosts, validate_media_url};
use super::write::{
    MIN_FREE_DISK_BYTES, PendingFile, disk_space_is_sufficient, effective_resume_offset,
    extension_from_url, random_task_path, write_chunks,
};
use super::*;
use crate::platforms;

fn allowed_hosts() -> HashSet<String> {
    normalize_allowed_hosts(platforms::wechat::REVIEWED_MEDIA_HOSTS.iter().copied()).unwrap()
}

#[test]
fn with_allowed_hosts_normalizes_and_exposes_the_set() {
    let downloader =
        MediaDownloader::with_allowed_hosts("media", ["Example.COM", "cdn.example.org"]).unwrap();

    assert!(downloader.allowed_hosts().contains("example.com"));
    assert!(downloader.allowed_hosts().contains("cdn.example.org"));
    assert_eq!(downloader.allowed_hosts().len(), 2);
}

#[test]
fn with_allowed_hosts_rejects_empty_or_invalid_entries() {
    assert!(matches!(
        MediaDownloader::with_allowed_hosts("media", std::iter::empty::<&str>()),
        Err(Error::Config(_))
    ));
    assert!(matches!(
        MediaDownloader::with_allowed_hosts("media", [".bad", "ok.example"]),
        Err(Error::Config(_))
    ));
    assert!(matches!(
        MediaDownloader::with_allowed_hosts("media", ["has space.example"]),
        Err(Error::Config(_))
    ));
}

#[test]
fn for_wechat_channels_uses_the_reviewed_host_set() {
    let downloader = MediaDownloader::for_wechat_channels("media").unwrap();
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
    let downloader = MediaDownloader::for_douyin("media").unwrap();
    for host in platforms::douyin::REVIEWED_MEDIA_HOSTS {
        assert!(
            downloader.allowed_hosts().contains(*host),
            "missing reviewed host {host}"
        );
    }
}

#[test]
fn for_platform_maps_known_ids() {
    let wechat = MediaDownloader::for_platform(crate::PlatformId::WechatChannels, "media").unwrap();
    assert!(wechat.allowed_hosts().contains("finder.video.qq.com"));
    let douyin = MediaDownloader::for_platform(crate::PlatformId::Douyin, "media").unwrap();
    assert!(douyin.allowed_hosts().contains("aweme.snssdk.com"));
    let _ = MediaDownloader::for_platform(crate::PlatformId::Bilibili, "media").unwrap();
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
    let path = random_task_path(
        Path::new("media"),
        &Url::parse("https://cdn.example/v.mp4").unwrap(),
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
        "https://finder.video.qq.com:444/path",
        "https://finder.video.qq.com/path#fragment",
        "https://finder.video.qq.com.evil.test/path",
        "https://finder.video.qq.com./path",
        "https://qq.com/path",
        "https://127.0.0.1/path",
    ] {
        let url = Url::parse(raw).unwrap();
        assert!(validate_media_url(&url, &allowed).is_err(), "{raw}");
    }
}

#[tokio::test]
async fn direct_url_download_still_rejects_disallowed_hosts_before_network_io() {
    let downloader = MediaDownloader::with_options(
        "media",
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
    for raw in ["::", "::1", "fc00::1", "fe80::1", "ff02::1", "2001:db8::1"] {
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
    let (reporter, guard) = ProgressReporter::new(Some(8), Some(callback));
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
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .map(|event| (event.percent, event.downloaded_bytes, event.total_bytes))
            .collect::<Vec<_>>(),
        [(20, 2, 8), (40, 6, 8), (60, 6, 8), (80, 8, 8)]
    );

    outcome
        .progress_reporter
        .as_mut()
        .unwrap()
        .report_complete(outcome.media.bytes);
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .map(|event| event.percent)
            .collect::<Vec<_>>(),
        [20, 40, 60, 80, 100]
    );

    drop(guard);
    drop(outcome);
    assert!(!path.exists());
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn does_not_report_percent_without_a_trusted_total() {
    let callback: ProgressCallback = Arc::new(|_| panic!("unexpected progress event"));
    let (reporter, guard) = ProgressReporter::new(None, Some(callback));
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
    let (mut reporter, guard) = ProgressReporter::new(Some(100), Some(callback));
    drop(guard);

    reporter.as_mut().unwrap().report_intermediate(75);
    reporter.as_mut().unwrap().report_complete(100);

    assert!(events.lock().unwrap().is_empty());
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
        "media",
        HashSet::from(["example.com".to_owned()]),
        Duration::from_secs(120),
        DownloadRequestIdentity::default(),
    )
    .unwrap();
    let tighter = downloader.with_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(tighter.request_timeout, Duration::from_secs(30));
    assert!(downloader.with_timeout(Duration::ZERO).is_err());
}
