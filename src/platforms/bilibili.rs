//! Bilibili public video resolve (BV / av / b23 short links).

use std::time::Duration;

use regex::Regex;
use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, ACCEPT_LANGUAGE, LOCATION, REFERER, USER_AGENT},
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
    url::{CleanPolicy, clean_tracking_params},
};

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_SHORTLINK_REDIRECTS: usize = 8;
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const VIEW_API: &str = "https://api.bilibili.com/x/web-interface/view";
const PLAYURL_API: &str = "https://api.bilibili.com/x/player/playurl";

/// Download allowlist. Entries starting with `.` are suffix rules.
pub const REVIEWED_MEDIA_HOSTS: &[&str] = &[
    ".bilivideo.com",
    ".bilivideo.cn",
    ".akamaized.net",
    "upos-sz-mirrorcos.bilivideo.com",
    "upos-sz-mirrorhw.bilivideo.com",
    "upos-sz-mirrorali.bilivideo.com",
    "upos-hz-mirrorakam.akamaized.net",
];

pub const REVIEWED_BILIBILI_MEDIA_HOSTS: &[&str] = REVIEWED_MEDIA_HOSTS;

pub fn download_identity() -> DownloadRequestIdentity {
    DownloadRequestIdentity {
        origin: Some("https://www.bilibili.com".into()),
        referer: Some("https://www.bilibili.com/".into()),
        user_agent: Some(USER_AGENT_VALUE.into()),
    }
}

#[derive(Clone)]
pub struct BilibiliResolver {
    client: Client,
    timeout: Duration,
}

impl std::fmt::Debug for BilibiliResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BilibiliResolver").finish_non_exhaustive()
    }
}

impl BilibiliResolver {
    pub fn new() -> Result<Self> {
        let timeout = RESOLVE_TIMEOUT;
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .redirect(Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| Error::Config("无法初始化哔哩哔哩 HTTP 客户端".into()))?;
        Ok(Self { client, timeout })
    }

    pub async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        let url = extract_share_url(input)?;
        self.resolve_url(&url).await
    }

    pub async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        tokio::time::timeout(self.timeout, self.resolve_normalized(url))
            .await
            .map_err(|_| Error::Network("哔哩哔哩解析总超时".into()))?
    }

    async fn resolve_normalized(&self, url: &Url) -> Result<ResolvedPost> {
        let mut current = clean_tracking_params(url, CleanPolicy::SHARE_PAGE);
        if is_b23_host(&current) {
            current = self.expand_short_link(current).await?;
        }
        let id = parse_video_id(&current)?;
        let view = self.request_view(&id).await?;
        build_post_from_view(&id, &view, self).await
    }

    async fn expand_short_link(&self, mut current: Url) -> Result<Url> {
        for _ in 0..MAX_SHORTLINK_REDIRECTS {
            if !is_b23_host(&current) {
                return Ok(current);
            }
            let response = self
                .client
                .get(current.clone())
                .header(USER_AGENT, USER_AGENT_VALUE)
                .header(ACCEPT, "*/*")
                .send()
                .await
                .map_err(|e| map_network_error(&e, "短链请求超时", "无法展开 b23 短链"))?;
            if !response.status().is_redirection() {
                return Err(Error::UpstreamChanged);
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(Error::UpstreamChanged)?;
            current = current.join(location).map_err(|_| Error::UpstreamChanged)?;
            current = clean_tracking_params(&current, CleanPolicy::SHARE_PAGE);
        }
        Err(Error::UpstreamChanged)
    }

    async fn request_view(&self, id: &VideoId) -> Result<Value> {
        let mut endpoint = Url::parse(VIEW_API).expect("constant");
        match id {
            VideoId::Bvid(bvid) => {
                endpoint.query_pairs_mut().append_pair("bvid", bvid);
            }
            VideoId::Aid(aid) => {
                endpoint
                    .query_pairs_mut()
                    .append_pair("aid", &aid.to_string());
            }
        }
        let response = self
            .client
            .get(endpoint)
            .header(ACCEPT, "application/json")
            .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .header(REFERER, "https://www.bilibili.com/")
            .send()
            .await
            .map_err(|e| map_network_error(&e, "上游请求超时", "无法连接上游服务"))?;
        map_status(response.status())?;
        let bytes = read_body_limited(response, MAX_JSON_BYTES, |e| {
            map_network_error(e, "上游请求超时", "无法连接上游服务")
        })
        .await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::UpstreamChanged)?;
        let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
        if code != 0 {
            return if code == -404 || code == 62002 {
                Err(Error::NotFound)
            } else if code == -412 || code == -101 {
                Err(Error::LoginRequired)
            } else {
                Err(Error::UpstreamChanged)
            };
        }
        value.get("data").cloned().ok_or(Error::UpstreamChanged)
    }

    async fn request_playurl(&self, bvid: &str, cid: u64) -> Result<Value> {
        let mut endpoint = Url::parse(PLAYURL_API).expect("constant");
        endpoint
            .query_pairs_mut()
            .append_pair("bvid", bvid)
            .append_pair("cid", &cid.to_string())
            .append_pair("qn", "80")
            .append_pair("fnval", "1")
            .append_pair("fourk", "1");
        let response = self
            .client
            .get(endpoint)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .header(REFERER, "https://www.bilibili.com/")
            .send()
            .await
            .map_err(|e| map_network_error(&e, "上游请求超时", "无法连接上游服务"))?;
        map_status(response.status())?;
        let bytes = read_body_limited(response, MAX_JSON_BYTES, |e| {
            map_network_error(e, "上游请求超时", "无法连接上游服务")
        })
        .await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::UpstreamChanged)?;
        let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
        if code != 0 {
            return Err(Error::MediaUnavailable);
        }
        value.get("data").cloned().ok_or(Error::MediaUnavailable)
    }
}

impl PlatformResolver for BilibiliResolver {
    fn platform_id(&self) -> PlatformId {
        PlatformId::Bilibili
    }

    fn extract_share_url(&self, input: &str) -> Result<Url> {
        extract_share_url(input)
    }

    async fn resolve_text(&self, input: &str) -> Result<ResolvedPost> {
        BilibiliResolver::resolve_text(self, input).await
    }

    async fn resolve_url(&self, url: &Url) -> Result<ResolvedPost> {
        BilibiliResolver::resolve_url(self, url).await
    }
}

#[derive(Debug, Clone)]
enum VideoId {
    Bvid(String),
    Aid(u64),
}

pub fn extract_share_url(input: &str) -> Result<Url> {
    static BV_PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static AV_PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static B23_PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();

    let bv = BV_PATTERN.get_or_init(|| {
        Regex::new(r#"https?://(?:www\.)?bilibili\.com/video/(BV[0-9A-Za-z]+)"#).expect("bv regex")
    });
    let av = AV_PATTERN.get_or_init(|| {
        Regex::new(r#"https?://(?:www\.)?bilibili\.com/video/av(\d+)"#).expect("av regex")
    });
    let b23 = B23_PATTERN
        .get_or_init(|| Regex::new(r#"https?://b23\.tv/[A-Za-z0-9]+"#).expect("b23 regex"));

    for matched in bv.find_iter(input) {
        let candidate = trim_url_candidate(matched.as_str());
        if let Ok(url) = Url::parse(candidate)
            && parse_video_id(&url).is_ok()
        {
            return Ok(clean_tracking_params(&url, CleanPolicy::SHARE_PAGE));
        }
    }
    for matched in av.find_iter(input) {
        let candidate = trim_url_candidate(matched.as_str());
        if let Ok(url) = Url::parse(candidate)
            && parse_video_id(&url).is_ok()
        {
            return Ok(clean_tracking_params(&url, CleanPolicy::SHARE_PAGE));
        }
    }
    for matched in b23.find_iter(input) {
        let candidate = trim_url_candidate(matched.as_str());
        if let Ok(url) = Url::parse(candidate) {
            return Ok(url);
        }
    }
    Err(Error::UnsupportedUrl)
}

fn parse_video_id(url: &Url) -> Result<VideoId> {
    let host = url
        .host_str()
        .map(|h| h.to_ascii_lowercase())
        .ok_or(Error::UnsupportedUrl)?;
    if host == "b23.tv" {
        return Err(Error::UnsupportedUrl);
    }
    if !matches!(
        host.as_str(),
        "www.bilibili.com" | "bilibili.com" | "m.bilibili.com"
    ) {
        return Err(Error::UnsupportedUrl);
    }
    let path = url.path();
    let Some(rest) = path.strip_prefix("/video/") else {
        return Err(Error::UnsupportedUrl);
    };
    let id = rest.split(['/', '?']).next().unwrap_or("");
    if id.starts_with("BV") && id.len() >= 6 {
        return Ok(VideoId::Bvid(id.to_owned()));
    }
    if let Some(aid) = id
        .strip_prefix("av")
        .or_else(|| id.strip_prefix("AV"))
        .and_then(|value| value.parse().ok())
    {
        return Ok(VideoId::Aid(aid));
    }
    Err(Error::UnsupportedUrl)
}

fn is_b23_host(url: &Url) -> bool {
    url.host_str()
        .is_some_and(|h| h.eq_ignore_ascii_case("b23.tv"))
}

async fn build_post_from_view(
    id: &VideoId,
    view: &Value,
    resolver: &BilibiliResolver,
) -> Result<ResolvedPost> {
    let bvid = view
        .get("bvid")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .or_else(|| match id {
            VideoId::Bvid(b) => Some(b.clone()),
            VideoId::Aid(_) => None,
        })
        .ok_or(Error::UpstreamChanged)?;
    let aid = view.get("aid").and_then(Value::as_u64);
    let title = view
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    let cover_url = view
        .get("pic")
        .and_then(Value::as_str)
        .and_then(|raw| Url::parse(raw).ok())
        .filter(|u| u.scheme() == "https");
    let cid = view
        .get("cid")
        .and_then(Value::as_u64)
        .or_else(|| view.pointer("/pages/0/cid").and_then(Value::as_u64))
        .ok_or(Error::UpstreamChanged)?;

    let play = resolver.request_playurl(&bvid, cid).await?;
    let sources = collect_durl_sources(&play)?;
    if sources.is_empty() {
        return Err(Error::MediaUnavailable);
    }
    let mut sources = sources;
    // Higher quality first (qn descending already from API order often).
    let primary = sources.remove(0);
    let fallbacks = sources;

    let post_id = aid.map(|a| a.to_string()).unwrap_or_else(|| bvid.clone());
    let canonical_url =
        Url::parse(&format!("https://www.bilibili.com/video/{bvid}")).expect("bvid url");

    Ok(ResolvedPost::new_video(
        PlatformId::Bilibili,
        post_id,
        canonical_url,
        title,
        cover_url,
        primary,
        fallbacks,
    ))
}

fn collect_durl_sources(play: &Value) -> Result<Vec<MediaSource>> {
    let mut sources = Vec::new();
    if let Some(durl) = play.get("durl").and_then(Value::as_array) {
        for item in durl {
            if let Some(source) = durl_item_to_source(item) {
                sources.push(source);
            }
        }
    }
    // Some responses nest under data.durl already unwrapped.
    Ok(sources)
}

fn durl_item_to_source(item: &Value) -> Option<MediaSource> {
    let raw = item
        .get("url")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())?;
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?;
    if !host_allowed(host) {
        return None;
    }
    let size_hint = item.get("size").and_then(Value::as_u64);
    Some(MediaSource {
        url,
        codec: VideoCodec::Unknown,
        provenance: MediaSourceKind::Direct,
        width: None,
        height: None,
        size_hint,
        decode_key: None,
    })
}

fn host_allowed(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    REVIEWED_MEDIA_HOSTS.iter().any(|entry| {
        if let Some(suffix) = entry.strip_prefix('.') {
            host == suffix || host.ends_with(&format!(".{suffix}")) || host.ends_with(suffix)
        } else {
            host == *entry
        }
    })
}

fn map_status(status: StatusCode) -> Result<()> {
    match status {
        s if s.is_success() => Ok(()),
        StatusCode::TOO_MANY_REQUESTS => Err(Error::RateLimited),
        StatusCode::NOT_FOUND | StatusCode::GONE => Err(Error::NotFound),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Error::LoginRequired),
        _ => Err(Error::Network(format!("上游返回 HTTP {}", status.as_u16()))),
    }
}

/// Build post from fixture view + play JSON (tests / offline).
pub fn build_post_from_fixtures(view: &Value, play: &Value) -> Result<ResolvedPost> {
    let bvid = view
        .get("bvid")
        .and_then(Value::as_str)
        .ok_or(Error::UpstreamChanged)?;
    let title = view.get("title").and_then(Value::as_str).map(str::to_owned);
    let cover_url = view
        .get("pic")
        .and_then(Value::as_str)
        .and_then(|raw| Url::parse(raw).ok());
    let mut sources = collect_durl_sources(play)?;
    if sources.is_empty() {
        return Err(Error::MediaUnavailable);
    }
    let primary = sources.remove(0);
    let post_id = view
        .get("aid")
        .and_then(Value::as_u64)
        .map(|a| a.to_string())
        .unwrap_or_else(|| bvid.to_owned());
    Ok(ResolvedPost::new_video(
        PlatformId::Bilibili,
        post_id,
        Url::parse(&format!("https://www.bilibili.com/video/{bvid}")).unwrap(),
        title,
        cover_url,
        primary,
        sources,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bv_and_av() {
        let bv = extract_share_url("看 https://www.bilibili.com/video/BV1GJ411x7h7?spm=1").unwrap();
        assert!(bv.path().contains("BV1GJ411x7h7"));
        assert!(bv.query().is_none() || !bv.query().unwrap().contains("utm_"));

        let av = extract_share_url("https://www.bilibili.com/video/av170001").unwrap();
        assert!(av.as_str().contains("av170001") || av.as_str().contains("170001"));
    }

    #[test]
    fn rejects_unrelated() {
        assert!(matches!(
            extract_share_url("https://www.example.com/video/BV1xx"),
            Err(Error::UnsupportedUrl)
        ));
    }

    #[test]
    fn builds_from_fixture_json() {
        let view = serde_json::json!({
            "bvid": "BV1GJ411x7h7",
            "aid": 170001,
            "title": "测试稿件",
            "pic": "https://i0.hdslb.com/bfs/cover.jpg",
            "cid": 280147
        });
        let play = serde_json::json!({
            "durl": [{
                "url": "https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/xx.mp4?deadline=1",
                "size": 12345
            }]
        });
        let post = build_post_from_fixtures(&view, &play).unwrap();
        assert_eq!(post.platform, PlatformId::Bilibili);
        assert_eq!(post.title.as_deref(), Some("测试稿件"));
        assert!(
            post.primary_video()
                .unwrap()
                .url
                .as_str()
                .contains("bilivideo.com")
        );
        assert_eq!(post.primary_video().unwrap().size_hint, Some(12345));
    }
}
