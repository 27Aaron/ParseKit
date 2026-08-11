//! Build normalized posts from Yuanbao parse and feed responses.

use serde_json::Value;
use url::Url;

use crate::{
    Error, PlatformId, Result,
    model::{MediaSource, MediaSourceKind, ResolvedPost, VideoCodec},
};

use super::share::{
    NormalizedShareUrl, derive_direct_media_url, is_allowed_media_url, query_value,
};
use super::util::{ParseData, non_empty, number_at, text_at};

pub(super) fn build_post(
    normalized: NormalizedShareUrl,
    parse_data: ParseData,
    feed: Value,
    export_id: String,
) -> Result<ResolvedPost> {
    let data = if feed.get("feedInfo").is_some() {
        &feed
    } else {
        feed.get("data")
            .and_then(|value| {
                if value.get("feedInfo").is_some() {
                    Some(value)
                } else {
                    value.get("data")
                }
            })
            .ok_or(Error::UpstreamChanged)?
    };
    let feed_info = data.get("feedInfo").ok_or(Error::UpstreamChanged)?;

    let mut candidates = Vec::new();
    push_candidate(
        &mut candidates,
        feed_info.get("h264VideoInfo"),
        MediaSourceKind::H264,
        VideoCodec::H264,
    );
    push_candidate(
        &mut candidates,
        feed_info.get("h265VideoInfo"),
        MediaSourceKind::H265,
        VideoCodec::H265,
    );
    push_source(
        &mut candidates,
        feed_info,
        "videoUrl",
        MediaSourceKind::Generic,
        VideoCodec::Unknown,
    );

    let direct_source = parse_source(
        feed_info,
        "originVideoUrl",
        MediaSourceKind::Direct,
        VideoCodec::Unknown,
    )
    .map(|mut direct| {
        if let Some(candidate) = matching_candidate(&direct, &candidates) {
            let same_url = direct.url == candidate.url;
            let share_key = may_share_decode_key(&direct, candidate);
            merge_source_metadata(&mut direct, candidate, same_url, share_key);
        }
        direct
    });

    let mut derived_sources = Vec::new();
    for candidate in [
        MediaSourceKind::H264,
        MediaSourceKind::H265,
        MediaSourceKind::Generic,
    ]
    .into_iter()
    .filter_map(|kind| candidates.iter().find(|source| source.provenance == kind))
    {
        if let Some(url) = derive_direct_media_url(&candidate.url) {
            let source = MediaSource {
                url,
                codec: candidate.codec,
                provenance: MediaSourceKind::Derived,
                width: candidate.width,
                height: candidate.height,
                // Retain the candidate size because source ranking uses it.
                size_hint: candidate.size_hint,
                decode_key: candidate.decode_key,
            };
            if !derived_sources
                .iter()
                .any(|existing: &MediaSource| sources_are_equivalent(existing, &source))
            {
                derived_sources.push(source);
            }
        }
    }
    sort_sources_by_quality(&mut derived_sources);

    let (video, fallback_videos) = if let Some(direct_source) = direct_source {
        let mut fallback_videos = derived_sources
            .into_iter()
            .filter(|source| !sources_are_equivalent(source, &direct_source))
            .collect::<Vec<_>>();
        sort_sources_by_quality(&mut fallback_videos);
        (direct_source, fallback_videos)
    } else {
        if derived_sources.is_empty() {
            return Err(Error::MediaUnavailable);
        }
        let video = derived_sources.remove(0);
        (video, derived_sources)
    };

    let title = text_at(feed_info, "description").or_else(|| non_empty(parse_data.desc));
    let cover_url = text_at(feed_info, "coverUrl")
        .or_else(|| non_empty(parse_data.cover_url))
        .and_then(|raw| Url::parse(&raw).ok())
        .filter(is_allowed_media_url);
    Ok(ResolvedPost::new_video(
        PlatformId::Wechat,
        non_empty(export_id).unwrap_or(normalized.share_id),
        normalized.canonical_url,
        title,
        cover_url,
        video,
        fallback_videos,
    ))
}

pub(super) fn push_candidate(
    output: &mut Vec<MediaSource>,
    value: Option<&Value>,
    kind: MediaSourceKind,
    codec: VideoCodec,
) {
    let Some(value) = value else { return };
    push_source(output, value, "videoUrl", kind, codec);
}

pub(super) fn push_source(
    output: &mut Vec<MediaSource>,
    value: &Value,
    url_field: &str,
    kind: MediaSourceKind,
    codec: VideoCodec,
) {
    let Some(source) = parse_source(value, url_field, kind, codec) else {
        return;
    };
    if let Some(existing) = output
        .iter_mut()
        .find(|existing| sources_are_equivalent(existing, &source))
    {
        merge_source_metadata(existing, &source, true, true);
    } else {
        output.push(source);
    }
}

pub(super) fn parse_source(
    value: &Value,
    url_field: &str,
    kind: MediaSourceKind,
    codec: VideoCodec,
) -> Option<MediaSource> {
    let raw_url = text_at(value, url_field)?;
    let url = Url::parse(&raw_url).ok()?;
    if !is_allowed_media_url(&url) {
        return None;
    }

    Some(MediaSource {
        url,
        codec,
        provenance: kind,
        width: number_at(value, "width").and_then(|value| u32::try_from(value).ok()),
        height: number_at(value, "height").and_then(|value| u32::try_from(value).ok()),
        size_hint: number_at(value, "fileSize"),
        decode_key: text_at(value, "decodeKey")
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| number_at(value, "decodeKey")),
    })
}

pub(super) fn matching_candidate<'a>(
    direct: &MediaSource,
    candidates: &'a [MediaSource],
) -> Option<&'a MediaSource> {
    if let Some(candidate) =
        unique_matching_candidate(candidates, |candidate| candidate.url == direct.url)
    {
        return Some(candidate);
    }

    if let Some(direct_url) = derive_direct_media_url(&direct.url)
        && let Some(candidate) = unique_matching_candidate(candidates, |candidate| {
            derive_direct_media_url(&candidate.url).as_ref() == Some(&direct_url)
        })
    {
        return Some(candidate);
    }

    unique_matching_candidate(candidates, |candidate| {
        has_matching_media_identity(&direct.url, &candidate.url)
    })
}

pub(super) fn sources_are_equivalent(left: &MediaSource, right: &MediaSource) -> bool {
    left.url == right.url && left.decode_key == right.decode_key
}

/// Ranks by resolution, size, then codec: H.264, H.265, unknown.
pub(super) fn quality_key(source: &MediaSource) -> (u64, u64, u8) {
    let pixels =
        u64::from(source.width.unwrap_or(0)).saturating_mul(u64::from(source.height.unwrap_or(0)));
    let size = source.size_hint.unwrap_or(0);
    let codec = match source.codec {
        VideoCodec::H264 => 2,
        VideoCodec::H265 => 1,
        VideoCodec::Unknown => 0,
    };
    (pixels, size, codec)
}

pub(super) fn sort_sources_by_quality(sources: &mut [MediaSource]) {
    sources.sort_by_key(|source| std::cmp::Reverse(quality_key(source)));
}

/// Returns whether `target` may safely inherit `source.decode_key`.
pub(super) fn may_share_decode_key(target: &MediaSource, source: &MediaSource) -> bool {
    if target.url == source.url {
        return true;
    }
    if let (Some(left), Some(right)) = (
        derive_direct_media_url(&target.url),
        derive_direct_media_url(&source.url),
    ) && left == right
    {
        return true;
    }
    // The caller guarantees a unique identity match, so key sharing is unambiguous.
    has_matching_media_identity(&target.url, &source.url)
}

pub(super) fn unique_matching_candidate(
    candidates: &[MediaSource],
    predicate: impl Fn(&MediaSource) -> bool,
) -> Option<&MediaSource> {
    let mut matches = candidates.iter().filter(|candidate| predicate(candidate));
    let candidate = matches.next()?;
    matches.next().is_none().then_some(candidate)
}

pub(super) fn has_matching_media_identity(left: &Url, right: &Url) -> bool {
    if left.scheme() != right.scheme()
        || left.host_str() != right.host_str()
        || left.port_or_known_default() != right.port_or_known_default()
        || left.path() != right.path()
    {
        return false;
    }

    let left_file_key = query_value(left, "encfilekey");
    let right_file_key = query_value(right, "encfilekey");
    let left_token = query_value(left, "token");
    let right_token = query_value(right, "token");
    let file_key_matches = matches!(
        (&left_file_key, &right_file_key),
        (Some(left), Some(right)) if left == right
    );
    let token_matches = matches!(
        (&left_token, &right_token),
        (Some(left), Some(right)) if left == right
    );
    let file_key_conflicts = matches!(
        (&left_file_key, &right_file_key),
        (Some(left), Some(right)) if left != right
    );
    let token_conflicts = matches!(
        (&left_token, &right_token),
        (Some(left), Some(right)) if left != right
    );

    !file_key_conflicts && !token_conflicts && (file_key_matches || token_matches)
}

pub(super) fn merge_source_metadata(
    target: &mut MediaSource,
    source: &MediaSource,
    inherit_size_hint: bool,
    inherit_decode_key: bool,
) {
    if target.codec == VideoCodec::Unknown {
        target.codec = source.codec;
    }
    if target.width.is_none() {
        target.width = source.width;
    }
    if target.height.is_none() {
        target.height = source.height;
    }
    // Never replace an existing decode key with a conflicting value.
    if inherit_decode_key {
        match (target.decode_key, source.decode_key) {
            (None, Some(key)) => target.decode_key = Some(key),
            (Some(existing), Some(other)) if existing != other => {}
            _ => {}
        }
    }
    if inherit_size_hint && target.size_hint.is_none() {
        target.size_hint = source.size_hint;
    }
}
