//! Douyin: short link → aweme_id → share page `_ROUTER_DATA`.

use std::time::Duration;

use regex::Regex;
use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, ACCEPT_LANGUAGE, LOCATION, USER_AGENT},
    redirect::Policy,
};
use serde_json::Value;
use url::Url;

use crate::{
    Error, PlatformId, Result,
    media::DownloadRequestIdentity,
    model::{MediaSource, MediaSourceKind, ResolvedPost, VideoCodec},
    platforms::{
        PlatformResolver,
        util::{map_network_error, read_body_limited, trim_url_candidate},
    },
};

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SHORTLINK_REDIRECTS: usize = 8;
const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MOBILE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) \
    AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1";
const MEDIA_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const DOUYIN_ORIGIN: &str = "https://www.douyin.com";
const DOUYIN_REFERER: &str = "https://www.douyin.com/";

/// Download allowlist. Entries starting with `.` are suffix rules.
pub const REVIEWED_MEDIA_HOSTS: &[&str] = &[
    "aweme.snssdk.com",
    "www.douyin.com",
    "www.iesdouyin.com",
    ".douyinvod.com",
    ".douyincdn.com",
    ".bytevcloudcdn.com",
    ".bytecdn.cn",
    ".bytecdn.com",
    ".zjcdn.com",
    ".douyinpic.com",
    ".ibyteimg.com",
    ".pstatp.com",
];

pub const REVIEWED_DOUYIN_MEDIA_HOSTS: &[&str] = REVIEWED_MEDIA_HOSTS;

const REDIRECT_HOSTS: &[&str] = &[
    "v.douyin.com",
    "www.douyin.com",
    "m.douyin.com",
    "www.iesdouyin.com",
    "iesdouyin.com",
];

pub fn download_identity() -> DownloadRequestIdentity {
    DownloadRequestIdentity {
        origin: Some(DOUYIN_ORIGIN.to_owned()),
        referer: Some(DOUYIN_REFERER.to_owned()),
        user_agent: Some(MEDIA_USER_AGENT.to_owned()),
    }
}

fn map_douyin_network_error(error: &reqwest::Error) -> Error {
    map_network_error(error, "抖音请求超时", "抖音网络请求失败")
}

#[derive(Clone)]
pub struct DouyinResolver {
    client: Client,
    timeout: Duration,
}

impl std::fmt::Debug for DouyinResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DouyinResolver")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl DouyinResolver {
    pub fn new() -> Result<Self> {
        let timeout = RESOLVE_TIMEOUT;
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .redirect(Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| Error::Config("无法初始化抖音 HTTP 客户端".into()))?;
        Ok(Self { client, timeout })
    }

    pub async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        let url = extract_share_url(input)?;
        tokio::time::timeout(self.timeout, self.resolve_url_inner(&url))
            .await
            .map_err(|_| Error::Network("抖音解析总超时".into()))?
    }

    pub async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        let url = extract_share_url(url.as_str())?;
        tokio::time::timeout(self.timeout, self.resolve_url_inner(&url))
            .await
            .map_err(|_| Error::Network("抖音解析总超时".into()))?
    }

    async fn resolve_url_inner(&self, url: &Url) -> Result<ResolvedPost> {
        let expanded = self.expand_short_link(url).await?;
        let aweme_id = extract_aweme_id(expanded.as_str()).ok_or(Error::UnsupportedUrl)?;
        let html = self.fetch_share_html(&aweme_id).await?;
        let router = parse_router_data(&html)?;
        build_post_from_router(&aweme_id, &router)
    }

    async fn expand_short_link(&self, url: &Url) -> Result<Url> {
        if !is_short_link_host(url) {
            return Ok(url.clone());
        }

        let mut current = url.clone();
        for _ in 0..MAX_SHORTLINK_REDIRECTS {
            if !is_allowed_redirect_host(&current) {
                return Err(Error::Network("抖音短链跳转到了未允许的主机".into()));
            }

            let response = self
                .client
                .get(current.clone())
                .header(USER_AGENT, MOBILE_UA)
                .header(ACCEPT, "text/html,application/xhtml+xml")
                .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
                .send()
                .await
                .map_err(|error| map_douyin_network_error(&error))?;

            if !response.status().is_redirection() {
                return Ok(response.url().clone());
            }

            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| Error::Network("抖音短链缺少 Location".into()))?;
            current = current
                .join(location)
                .map_err(|_| Error::Network("抖音短链 Location 无效".into()))?;

            if extract_aweme_id(current.as_str()).is_some() {
                return Ok(current);
            }
        }

        Err(Error::Network("抖音短链重定向次数过多".into()))
    }

    async fn fetch_share_html(&self, aweme_id: &str) -> Result<String> {
        let share_url = Url::parse(&format!("https://www.iesdouyin.com/share/video/{aweme_id}"))
            .expect("constant share URL template is valid");

        let response = self
            .client
            .get(share_url)
            .header(USER_AGENT, MOBILE_UA)
            .header(ACCEPT, "text/html,application/xhtml+xml")
            .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.8")
            .send()
            .await
            .map_err(|error| map_douyin_network_error(&error))?;

        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(Error::RateLimited);
        }
        if status == StatusCode::NOT_FOUND {
            return Err(Error::NotFound);
        }
        if !status.is_success() {
            return Err(Error::Network(format!(
                "抖音分享页 HTTP {}",
                status.as_u16()
            )));
        }

        let bytes = read_body_limited(response, MAX_HTML_BYTES, map_douyin_network_error).await?;
        String::from_utf8(bytes).map_err(|_| Error::UpstreamChanged)
    }
}

impl PlatformResolver for DouyinResolver {
    fn platform_id(&self) -> PlatformId {
        PlatformId::Douyin
    }

    fn extract_share_url(&self, input: &str) -> Result<Url> {
        extract_share_url(input)
    }

    async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        DouyinResolver::resolve_text(self, input).await
    }

    async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        DouyinResolver::resolve_url(self, url).await
    }
}

pub fn extract_share_url(input: &str) -> Result<Url> {
    static URL_PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let pattern = URL_PATTERN.get_or_init(|| {
        Regex::new(
            r#"(?i)https?://(?:(?:v|www|m)\.)?douyin\.com/[^\s<>"']+|https?://(?:www\.)?iesdouyin\.com/[^\s<>"']+"#,
        )
        .expect("constant Douyin URL regex must compile")
    });

    for matched in pattern.find_iter(input) {
        let candidate = trim_url_candidate(matched.as_str());
        let Ok(mut url) = Url::parse(candidate) else {
            continue;
        };
        if url.scheme() != "http" && url.scheme() != "https" {
            continue;
        }
        if url.scheme() == "http" {
            let _ = url.set_scheme("https");
        }
        if is_douyin_host(&url) && !is_excluded_path(&url) {
            return Ok(url);
        }
    }
    Err(Error::UnsupportedUrl)
}

fn is_douyin_host(url: &Url) -> bool {
    let Some(host) = url.host_str().map(|h| h.to_ascii_lowercase()) else {
        return false;
    };
    matches!(
        host.as_str(),
        "v.douyin.com"
            | "www.douyin.com"
            | "m.douyin.com"
            | "douyin.com"
            | "www.iesdouyin.com"
            | "iesdouyin.com"
    )
}

fn is_short_link_host(url: &Url) -> bool {
    url.host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("v.douyin.com"))
}

fn is_allowed_redirect_host(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|host| {
            REDIRECT_HOSTS
                .iter()
                .any(|allowed| host.eq_ignore_ascii_case(allowed))
        })
}

fn is_excluded_path(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    path.starts_with("/share/user") || path.starts_with("/qishui") || path.starts_with("/user/")
}

fn extract_aweme_id(input: &str) -> Option<String> {
    static PATTERNS: std::sync::OnceLock<Vec<Regex>> = std::sync::OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            r"(?i)/(?:share/)?video/(\d{5,32})",
            r"(?i)/note/(\d{5,32})",
            r"(?i)[?&]modal_id=(\d{5,32})",
            r"(?i)[?&]vid=(\d{5,32})",
        ]
        .into_iter()
        .map(|pattern| Regex::new(pattern).expect("constant aweme id regex must compile"))
        .collect()
    });

    for pattern in patterns {
        if let Some(capture) = pattern.captures(input)
            && let Some(id) = capture.get(1)
        {
            return Some(id.as_str().to_owned());
        }
    }
    None
}

fn parse_router_data(html: &str) -> Result<Value> {
    // Slice JSON between assignment and </script> (nested braces break regex).
    const MARKER: &str = "window._ROUTER_DATA";
    let marker_at = html.find(MARKER).ok_or(Error::UpstreamChanged)?;
    let after_marker = &html[marker_at + MARKER.len()..];
    let eq_at = after_marker.find('=').ok_or(Error::UpstreamChanged)?;
    let after_eq = after_marker[eq_at + 1..].trim_start();
    let script_end = after_eq.find("</script>").ok_or(Error::UpstreamChanged)?;
    let mut json_slice = after_eq[..script_end].trim();
    if let Some(stripped) = json_slice.strip_suffix(';') {
        json_slice = stripped.trim();
    }
    if !json_slice.starts_with('{') {
        return Err(Error::UpstreamChanged);
    }
    serde_json::from_str(json_slice).map_err(|_| Error::UpstreamChanged)
}

fn build_post_from_router(aweme_id: &str, router: &Value) -> Result<ResolvedPost> {
    // Key is `video_(id)/page`; `/` → `~1` in JSON Pointer.
    let page = router
        .pointer("/loaderData/video_(id)~1page")
        .ok_or(Error::UpstreamChanged)?;
    let video_info = page.get("videoInfoRes").ok_or(Error::UpstreamChanged)?;

    if let Some(filter_list) = video_info.get("filter_list").and_then(Value::as_array)
        && !filter_list.is_empty()
    {
        let reason = filter_list
            .first()
            .and_then(|item| item.get("filter_reason"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return if reason.contains("NOT_EXIST") || reason.contains("DELETE") {
            Err(Error::NotFound)
        } else {
            Err(Error::MediaUnavailable)
        };
    }

    let item = video_info
        .get("item_list")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .ok_or(Error::NotFound)?;

    let post_id = item
        .get("aweme_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(aweme_id)
        .to_owned();

    let title = item
        .get("desc")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);

    let canonical_url = Url::parse(&format!("https://www.douyin.com/video/{post_id}"))
        .expect("aweme id forms a valid URL");

    // Image / note posts.
    if let Some(images) = collect_image_sources(item)
        && !images.is_empty()
    {
        let cover_url = images.first().map(|source| source.url.clone());
        return ResolvedPost::new_image_set(
            PlatformId::Douyin,
            post_id,
            canonical_url,
            title,
            cover_url,
            images,
        );
    }

    let video = item.get("video").ok_or(Error::MediaUnavailable)?;
    let mut sources = collect_video_sources(video)?;
    if sources.is_empty() {
        return Err(Error::MediaUnavailable);
    }
    let cover_url = pick_cover_url(video);
    let primary = sources.remove(0);
    Ok(ResolvedPost::new_video(
        PlatformId::Douyin,
        post_id,
        canonical_url,
        title,
        cover_url,
        primary,
        sources,
    ))
}

fn collect_image_sources(item: &Value) -> Option<Vec<MediaSource>> {
    let images = item.get("images").and_then(Value::as_array)?;
    if images.is_empty() {
        return None;
    }
    let mut sources = Vec::new();
    for image in images {
        let url_list = image
            .get("url_list")
            .or_else(|| image.pointer("/display_image/url_list"))
            .and_then(Value::as_array)?;
        let raw = url_list
            .iter()
            .filter_map(Value::as_str)
            .rev()
            .find(|value| !value.is_empty())?;
        let url = Url::parse(raw).ok()?;
        if url.scheme() != "https" {
            continue;
        }
        sources.push(MediaSource {
            url,
            codec: VideoCodec::Unknown,
            provenance: MediaSourceKind::Direct,
            width: image
                .get("width")
                .and_then(Value::as_u64)
                .map(|value| value as u32),
            height: image
                .get("height")
                .and_then(Value::as_u64)
                .map(|value| value as u32),
            size_hint: None,
            decode_key: None,
        });
    }
    (!sources.is_empty()).then_some(sources)
}

/// Ranked video qualities: highest first (primary + fallbacks).
fn collect_video_sources(video: &Value) -> Result<Vec<MediaSource>> {
    let mut ranked = Vec::new();

    if let Some(bit_rates) = video.get("bit_rate").and_then(Value::as_array) {
        for item in bit_rates {
            let play = item.get("play_addr").unwrap_or(item);
            if let Some(source) = play_addr_to_source(play) {
                ranked.push((quality_score(item, play), source));
            }
        }
    }

    if ranked.is_empty()
        && let Some(play_addr) = video.get("play_addr")
        && let Some(source) = play_addr_to_source(play_addr)
    {
        ranked.push((0, source));
    }

    if ranked.is_empty()
        && let Some(uri) = video
            .pointer("/play_addr/uri")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    {
        let width = video
            .pointer("/play_addr/width")
            .and_then(Value::as_u64)
            .map(|value| value as u32);
        let height = video
            .pointer("/play_addr/height")
            .and_then(Value::as_u64)
            .map(|value| value as u32);
        let url = Url::parse(&format!(
            "https://www.douyin.com/aweme/v1/play/?video_id={uri}&ratio=720p&line=0"
        ))
        .map_err(|_| Error::UpstreamChanged)?;
        ranked.push((
            0,
            MediaSource {
                url,
                codec: VideoCodec::Unknown,
                provenance: MediaSourceKind::Direct,
                width,
                height,
                size_hint: None,
                decode_key: None,
            },
        ));
    }

    ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let mut sources = Vec::new();
    for (_, source) in ranked {
        if sources
            .iter()
            .any(|existing: &MediaSource| existing.url == source.url)
        {
            continue;
        }
        sources.push(source);
    }
    if sources.is_empty() {
        Err(Error::MediaUnavailable)
    } else {
        Ok(sources)
    }
}

fn quality_score(item: &Value, play: &Value) -> u64 {
    let width = play.get("width").and_then(Value::as_u64).unwrap_or(0);
    let height = play.get("height").and_then(Value::as_u64).unwrap_or(0);
    let size = play.get("data_size").and_then(Value::as_u64).unwrap_or(0);
    let bitrate = item.get("bit_rate").and_then(Value::as_u64).unwrap_or(0);
    width
        .saturating_mul(height)
        .saturating_mul(1_000)
        .saturating_add(size)
        .saturating_add(bitrate)
}

fn play_addr_to_source(play_addr: &Value) -> Option<MediaSource> {
    let url_list = play_addr.get("url_list")?.as_array()?;
    let raw = url_list
        .iter()
        .filter_map(Value::as_str)
        .find(|value| !value.is_empty())?;
    let cleaned = remove_video_watermark(raw);
    let url = Url::parse(&cleaned).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let width = play_addr
        .get("width")
        .and_then(Value::as_u64)
        .map(|value| value as u32);
    let height = play_addr
        .get("height")
        .and_then(Value::as_u64)
        .map(|value| value as u32);
    let size_hint = play_addr.get("data_size").and_then(Value::as_u64);
    Some(MediaSource {
        url,
        codec: VideoCodec::Unknown,
        provenance: MediaSourceKind::Direct,
        width,
        height,
        size_hint,
        decode_key: None,
    })
}

fn remove_video_watermark(url: &str) -> String {
    url.replace("playwm", "play")
}

fn pick_cover_url(video: &Value) -> Option<Url> {
    let cover = video.get("cover").or_else(|| video.get("origin_cover"))?;
    let url_list = cover.get("url_list")?.as_array()?;
    let raw = url_list
        .iter()
        .filter_map(Value::as_str)
        .rev()
        .find(|value| !value.is_empty())?;
    let url = Url::parse(raw).ok()?;
    (url.scheme() == "https").then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_urls_from_share_text() {
        let cases = [
            (
                "打开抖音 https://v.douyin.com/iAbCdEf/ 看看",
                "https://v.douyin.com/iAbCdEf/",
            ),
            (
                "https://www.douyin.com/video/7123456789012345678",
                "https://www.douyin.com/video/7123456789012345678",
            ),
            (
                "https://www.iesdouyin.com/share/video/7123456789012345678/",
                "https://www.iesdouyin.com/share/video/7123456789012345678/",
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
    }

    #[test]
    fn extracts_aweme_ids() {
        assert_eq!(
            extract_aweme_id("https://www.douyin.com/video/7123456789012345678?x=1").as_deref(),
            Some("7123456789012345678")
        );
        assert_eq!(
            extract_aweme_id("https://www.iesdouyin.com/share/video/7123456789012345678/")
                .as_deref(),
            Some("7123456789012345678")
        );
        assert_eq!(
            extract_aweme_id("https://www.douyin.com/discover?modal_id=7123456789012345678")
                .as_deref(),
            Some("7123456789012345678")
        );
        assert_eq!(
            extract_aweme_id("https://www.douyin.com/note/7123456789012345678").as_deref(),
            Some("7123456789012345678")
        );
    }

    #[test]
    fn builds_post_from_fixture_item_list() {
        let router: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/douyin/router_video.json"
        ))
        .expect("committed douyin router fixture");

        let post = build_post_from_router("7123456789012345678", &router).unwrap();
        assert_eq!(post.platform, PlatformId::Douyin);
        assert_eq!(post.post_id, "7123456789012345678");
        assert_eq!(post.title.as_deref(), Some("测试标题"));
        assert!(
            post.primary_video()
                .unwrap()
                .url
                .as_str()
                .contains("play/?video_id=")
        );
        assert!(
            !post
                .primary_video()
                .unwrap()
                .url
                .as_str()
                .contains("playwm")
        );
        assert_eq!(post.primary_video().unwrap().width, Some(720));
        assert_eq!(post.primary_video().unwrap().height, Some(1280));
        assert!(post.cover_url.is_some());
    }

    #[test]
    fn parse_router_data_from_committed_html_fixture() {
        let html = include_str!("../../tests/fixtures/douyin/share_page_router.html");
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
                .unwrap()
                .url
                .as_str()
                .contains("douyinpic.com")
        );
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
                .unwrap()
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
    fn watermark_removal() {
        assert_eq!(
            remove_video_watermark("https://x/playwm/?v=1"),
            "https://x/play/?v=1"
        );
    }
}
