//! Map Bilibili API payloads into the shared media model.

use std::collections::HashSet;

use serde_json::Value;
use url::Url;

use crate::{
    Error, PlatformId, Result,
    media::host::is_reviewed_https_url,
    model::{MediaSource, MediaSourceKind, ResolvedPost, VideoCodec},
};

use super::{BilibiliResolver, REVIEWED_MEDIA_HOSTS, share::VideoId};

pub(super) async fn build_post_from_view(
    id: &VideoId,
    view: &Value,
    page: Option<usize>,
    resolver: &BilibiliResolver,
) -> Result<ResolvedPost> {
    let bvid = view
        .get("bvid")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| match id {
            VideoId::Bvid(bvid) => Some(bvid.clone()),
            VideoId::Aid(_) => None,
        })
        .ok_or(Error::UpstreamChanged)?;
    let aid = view.get("aid").and_then(Value::as_u64);
    let title = title_from_view(view);
    let cover_url = cover_from_view(view);
    let cid = cid_from_view(view, page)?;

    let sources = resolver.request_play_sources(&bvid, cid).await?;
    build_post_from_data(&bvid, aid, title, cover_url, page, sources)
}

pub(super) fn cid_from_view(view: &Value, page: Option<usize>) -> Result<u64> {
    if let Some(page) = page {
        if let Some(cid) = page
            .checked_sub(1)
            .and_then(|index| view.get("pages")?.as_array()?.get(index))
            .and_then(|page| page.get("cid"))
            .and_then(Value::as_u64)
        {
            return Ok(cid);
        }
        if page != 1 {
            return Err(Error::NotFound);
        }
    }

    view.get("cid")
        .and_then(Value::as_u64)
        .or_else(|| view.pointer("/pages/0/cid").and_then(Value::as_u64))
        .ok_or(Error::UpstreamChanged)
}

fn title_from_view(view: &Value) -> Option<String> {
    view.get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn cover_from_view(view: &Value) -> Option<Url> {
    view.get("pic")
        .and_then(Value::as_str)
        .and_then(|raw| Url::parse(raw).ok())
        .filter(|url| is_reviewed_https_url(url, REVIEWED_MEDIA_HOSTS))
}

fn build_post_from_data(
    bvid: &str,
    aid: Option<u64>,
    title: Option<String>,
    cover_url: Option<Url>,
    page: Option<usize>,
    mut sources: Vec<MediaSource>,
) -> Result<ResolvedPost> {
    if sources.is_empty() {
        return Err(Error::MediaUnavailable);
    }
    let primary = sources.remove(0);
    let post_id = aid.map_or_else(|| bvid.to_owned(), |aid| aid.to_string());
    let mut canonical_url = Url::parse(&format!("https://www.bilibili.com/video/{bvid}"))
        .map_err(|_| Error::UpstreamChanged)?;
    if let Some(page) = page.filter(|page| *page > 1) {
        canonical_url
            .query_pairs_mut()
            .append_pair("p", &page.to_string());
    }

    Ok(ResolvedPost::new_video(
        PlatformId::Bilibili,
        post_id,
        canonical_url,
        title,
        cover_url,
        primary,
        sources,
    ))
}

#[cfg(test)]
pub(super) fn build_post_from_payloads(view: &Value, play: &Value) -> Result<ResolvedPost> {
    build_post_from_payloads_for_page(view, play, None)
}

#[cfg(test)]
pub(super) fn build_post_from_payloads_for_page(
    view: &Value,
    play: &Value,
    page: Option<usize>,
) -> Result<ResolvedPost> {
    let bvid = view
        .get("bvid")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(Error::UpstreamChanged)?;
    build_post_from_data(
        bvid,
        view.get("aid").and_then(Value::as_u64),
        title_from_view(view),
        cover_from_view(view),
        page,
        collect_play_sources(play),
    )
}

pub(super) fn collect_play_sources(play: &Value) -> Vec<MediaSource> {
    let sources = collect_durl_sources(play);
    let mut seen = HashSet::with_capacity(sources.len());
    let mut unique = Vec::with_capacity(sources.len());
    for source in sources {
        if seen.insert(source.url.clone()) {
            unique.push(source);
        }
    }
    unique
}

fn collect_durl_sources(play: &Value) -> Vec<MediaSource> {
    let Some(durl) = play.get("durl").and_then(Value::as_array) else {
        return Vec::new();
    };
    // Multiple durl entries are sequential segments, not fallbacks.
    if durl.len() != 1 {
        return Vec::new();
    }

    let item = &durl[0];
    let size_hint = item.get("size").and_then(Value::as_u64);
    // Do not infer codecs absent from progressive metadata.
    let tier = play
        .get("quality")
        .and_then(Value::as_u64)
        .and_then(bilibili_qn_label);
    let codec_tag = play
        .get("video_codecid")
        .and_then(Value::as_u64)
        .and_then(bilibili_codecid_tag);
    let label = match (tier, codec_tag) {
        (Some(t), Some(c)) => Some(format!("{t}/{c}")),
        (Some(t), None) => Some(t.to_owned()),
        (None, Some(c)) => Some(c.to_owned()),
        (None, None) => None,
    };
    let codec = play
        .get("video_codecid")
        .and_then(Value::as_u64)
        .map(bilibili_codecid_to_codec)
        .unwrap_or(VideoCodec::Unknown);
    let mut sources = Vec::new();
    if let Some(mut source) = media_url_item_to_source(item, "url", MediaSourceKind::Direct) {
        source.label = label.clone();
        source.codec = codec;
        sources.push(source);
    }
    if let Some(backups) = item.get("backup_url").and_then(Value::as_array) {
        for backup in backups {
            if let Some(raw) = backup.as_str()
                && let Some(mut source) =
                    https_media_source(raw, size_hint, MediaSourceKind::Derived)
            {
                source.label = label.clone();
                source.codec = codec;
                sources.push(source);
            }
        }
    }
    sources
}

/// Maps known `video_codecid` values to display tags.
fn bilibili_codecid_tag(codecid: u64) -> Option<&'static str> {
    match codecid {
        7 => Some("AVC"),
        12 => Some("HEVC"),
        13 => Some("AV1"),
        _ => None,
    }
}

fn bilibili_codecid_to_codec(codecid: u64) -> VideoCodec {
    match codecid {
        7 => VideoCodec::H264,
        12 => VideoCodec::H265,
        _ => VideoCodec::Unknown,
    }
}

fn bilibili_qn_label(qn: u64) -> Option<&'static str> {
    Some(match qn {
        127 => "8K",
        126 => "Dolby",
        125 => "HDR",
        120 => "4K",
        116 => "1080P60",
        112 => "1080P+",
        80 => "1080P",
        74 => "720P60",
        64 => "720P",
        32 => "480P",
        16 => "360P",
        _ => return None,
    })
}

fn media_url_item_to_source(
    item: &Value,
    url_key: &str,
    kind: MediaSourceKind,
) -> Option<MediaSource> {
    let raw = item
        .get(url_key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())?;
    let size_hint = item.get("size").and_then(Value::as_u64);
    https_media_source(raw, size_hint, kind)
}

fn https_media_source(
    raw: &str,
    size_hint: Option<u64>,
    kind: MediaSourceKind,
) -> Option<MediaSource> {
    let url = Url::parse(raw).ok()?;
    if !is_reviewed_https_url(&url, REVIEWED_MEDIA_HOSTS) {
        return None;
    }
    Some(MediaSource {
        url,
        codec: VideoCodec::Unknown,
        provenance: kind,
        width: None,
        height: None,
        size_hint,
        decode_key: None,
        label: None,
        bitrate_bps: None,
    })
}
