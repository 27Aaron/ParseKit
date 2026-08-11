//! Map embedded Douyin page data into the shared media model.

use serde_json::Value;
use url::Url;

use crate::{
    Error, PlatformId, Result,
    media::host::is_reviewed_https_url,
    model::{MediaSource, MediaSourceKind, ResolvedPost, VideoCodec},
};

use super::REVIEWED_MEDIA_HOSTS;

pub(super) fn parse_router_data(html: &str) -> Result<Value> {
    parse_script_json_assignment(html, "window._ROUTER_DATA")
}

pub(super) fn parse_any_page_data(html: &str) -> Result<Value> {
    for marker in [
        "window._ROUTER_DATA",
        "window.__RENDER_DATA__",
        "window.RENDER_DATA",
    ] {
        if let Ok(value) = parse_script_json_assignment(html, marker) {
            return Ok(value);
        }
    }
    Err(Error::UpstreamChanged)
}

fn parse_script_json_assignment(html: &str, marker: &str) -> Result<Value> {
    // Slice through `</script>` because a regex cannot safely match nested JSON.
    let marker_at = html.find(marker).ok_or(Error::UpstreamChanged)?;
    let after_marker = &html[marker_at + marker.len()..];
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

pub(super) fn build_post_from_router(aweme_id: &str, router: &Value) -> Result<ResolvedPost> {
    // JSON Pointer escapes the slash in `video_(id)/page` as `~1`.
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
        .map_err(|_| Error::UpstreamChanged)?;

    if let Some(images) = collect_image_sources(item) {
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
    let mut sources = Vec::with_capacity(images.len());
    for image in images {
        let Some(url_list) = image
            .get("url_list")
            .or_else(|| image.pointer("/display_image/url_list"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        let Some(url) = url_list
            .iter()
            .filter_map(Value::as_str)
            .rev()
            .filter_map(|raw| Url::parse(raw).ok())
            .find(|url| is_reviewed_https_url(url, REVIEWED_MEDIA_HOSTS))
        else {
            continue;
        };
        sources.push(MediaSource {
            url,
            codec: VideoCodec::Unknown,
            provenance: MediaSourceKind::Direct,
            width: image
                .get("width")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok()),
            height: image
                .get("height")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok()),
            size_hint: None,
            decode_key: None,
        });
    }
    (!sources.is_empty()).then_some(sources)
}

/// Collects video sources in descending quality order.
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
            .and_then(|value| u32::try_from(value).ok());
        let height = video
            .pointer("/play_addr/height")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
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
    let url = play_addr
        .get("url_list")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .filter(|raw| !raw.is_empty())
        .filter_map(|raw| Url::parse(raw).ok())
        .map(remove_video_watermark)
        .find(|url| is_reviewed_https_url(url, REVIEWED_MEDIA_HOSTS))?;
    let width = play_addr
        .get("width")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let height = play_addr
        .get("height")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
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

pub(super) fn remove_video_watermark(mut url: Url) -> Url {
    let clean_path = url
        .path()
        .split('/')
        .map(|segment| if segment == "playwm" { "play" } else { segment })
        .collect::<Vec<_>>()
        .join("/");
    url.set_path(&clean_path);
    url
}

fn pick_cover_url(video: &Value) -> Option<Url> {
    let cover = video.get("cover").or_else(|| video.get("origin_cover"))?;
    cover
        .get("url_list")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .rev()
        .filter_map(|raw| Url::parse(raw).ok())
        .find(|url| is_reviewed_https_url(url, REVIEWED_MEDIA_HOSTS))
}
