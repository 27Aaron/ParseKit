//! Map Bilibili API payloads into the shared media model.

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
    let cid = view
        .get("cid")
        .and_then(Value::as_u64)
        .or_else(|| view.pointer("/pages/0/cid").and_then(Value::as_u64))
        .ok_or(Error::UpstreamChanged)?;

    let sources = resolver.request_play_sources(&bvid, cid).await?;
    build_post_from_data(&bvid, aid, title, cover_url, sources)
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
    mut sources: Vec<MediaSource>,
) -> Result<ResolvedPost> {
    if sources.is_empty() {
        return Err(Error::MediaUnavailable);
    }
    let primary = sources.remove(0);
    let post_id = aid.map_or_else(|| bvid.to_owned(), |aid| aid.to_string());
    let canonical_url = Url::parse(&format!("https://www.bilibili.com/video/{bvid}"))
        .map_err(|_| Error::UpstreamChanged)?;

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
        collect_play_sources(play),
    )
}

pub(super) fn collect_play_sources(play: &Value) -> Vec<MediaSource> {
    let mut sources = collect_durl_sources(play);
    sources.extend(collect_dash_video_sources(play));
    let mut unique = Vec::with_capacity(sources.len());
    for source in sources {
        if !unique
            .iter()
            .any(|existing: &MediaSource| existing.url == source.url)
        {
            unique.push(source);
        }
    }
    unique
}

fn collect_durl_sources(play: &Value) -> Vec<MediaSource> {
    let Some(durl) = play.get("durl").and_then(Value::as_array) else {
        return Vec::new();
    };
    // Multiple `durl` entries are sequential segments, not fallbacks. Reject
    // them until the model can represent a multi-part video.
    if durl.len() != 1 {
        return Vec::new();
    }

    let item = &durl[0];
    let size_hint = item.get("size").and_then(Value::as_u64);
    // Progressive playurl requests typically target 1080P (qn=80) first; label
    // from quality when present, otherwise leave unset for display fallback.
    let qn_label = play
        .get("quality")
        .and_then(Value::as_u64)
        .and_then(bilibili_qn_label)
        .map(str::to_owned);
    let mut sources = Vec::new();
    if let Some(mut source) = media_url_item_to_source(item, "url", MediaSourceKind::Direct) {
        source.label = qn_label.clone();
        sources.push(source);
    }
    if let Some(backups) = item.get("backup_url").and_then(Value::as_array) {
        for backup in backups {
            if let Some(raw) = backup.as_str()
                && let Some(mut source) =
                    https_media_source(raw, size_hint, MediaSourceKind::Derived)
            {
                source.label = qn_label.clone();
                sources.push(source);
            }
        }
    }
    sources
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

fn collect_dash_video_sources(play: &Value) -> Vec<MediaSource> {
    let mut ranked: Vec<(u64, MediaSource)> = Vec::new();
    let Some(videos) = play.pointer("/dash/video").and_then(Value::as_array) else {
        return Vec::new();
    };

    for item in videos {
        let bandwidth = item.get("bandwidth").and_then(Value::as_u64).unwrap_or(0);
        let width = item
            .get("width")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
        let height = item
            .get("height")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
        let size_hint = item.get("size").and_then(Value::as_u64);
        for key in ["base_url", "baseUrl", "url"] {
            if let Some(raw) = item.get(key).and_then(Value::as_str)
                && let Some(mut source) =
                    https_media_source(raw, size_hint, MediaSourceKind::Derived)
            {
                source.width = width;
                source.height = height;
                source.bitrate_bps = (bandwidth > 0).then_some(bandwidth);
                source.label =
                    crate::model::resolution_tier_label(width, height).map(str::to_owned);
                ranked.push((bandwidth, source));
                break;
            }
        }
        if let Some(backups) = item
            .get("backup_url")
            .or_else(|| item.get("backupUrl"))
            .and_then(Value::as_array)
        {
            for backup in backups {
                if let Some(raw) = backup.as_str()
                    && let Some(mut source) =
                        https_media_source(raw, size_hint, MediaSourceKind::Derived)
                {
                    source.width = width;
                    source.height = height;
                    source.bitrate_bps = (bandwidth > 0).then_some(bandwidth);
                    source.label =
                        crate::model::resolution_tier_label(width, height).map(str::to_owned);
                    ranked.push((bandwidth.saturating_sub(1), source));
                }
            }
        }
    }
    ranked.sort_by_key(|(bandwidth, _)| std::cmp::Reverse(*bandwidth));
    ranked.into_iter().map(|(_, source)| source).collect()
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
