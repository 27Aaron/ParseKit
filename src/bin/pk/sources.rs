//! Select media sources for CLI downloads.

use parse_kit::{ContentKind, Error, MediaSource, ResolvedPost, Result};

use crate::args::Prefer;

pub fn requested_source_index(
    kind: ContentKind,
    source: Option<usize>,
    first_only: bool,
) -> Option<usize> {
    source.or_else(|| (kind == ContentKind::ImageSet && first_only).then_some(0))
}

pub fn select_sources(
    post: &ResolvedPost,
    prefer: Prefer,
    source: Option<usize>,
) -> Result<Vec<&MediaSource>> {
    let mut sources: Vec<&MediaSource> = post.media_sources().collect();
    if sources.is_empty() {
        return Err(Error::MediaUnavailable);
    }

    // Explicit indices always refer to `pk resolve` order. `--prefer` only
    // reorders automatic fallbacks.
    if let Some(index) = source {
        let chosen = sources.get(index).copied().ok_or_else(|| {
            Error::Config(format!(
                "无效的 --source {index}（可用 0..{}）",
                sources.len().saturating_sub(1)
            ))
        })?;
        return Ok(vec![chosen]);
    }

    match prefer {
        Prefer::Best => {}
        Prefer::Smallest => sources.reverse(),
    }

    Ok(sources)
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;
    use parse_kit::{MediaSourceKind, PlatformId, VideoCodec};

    fn source(name: &str) -> MediaSource {
        MediaSource {
            url: Url::parse(&format!("https://media.example/{name}.mp4")).expect("test URL"),
            codec: VideoCodec::H264,
            provenance: MediaSourceKind::Direct,
            width: None,
            height: None,
            size_hint: None,
            decode_key: None,
            label: None,
            bitrate_bps: None,
        }
    }

    fn video_post() -> ResolvedPost {
        ResolvedPost::new_video(
            PlatformId::Douyin,
            "7661946724177829115",
            Url::parse("https://www.douyin.com/video/7661946724177829115").expect("test URL"),
            None,
            None,
            source("best"),
            vec![source("middle"), source("smallest")],
        )
    }

    #[test]
    fn preference_changes_only_automatic_fallback_order() {
        let post = video_post();
        let best = select_sources(&post, Prefer::Best, None).expect("sources");
        let smallest = select_sources(&post, Prefer::Smallest, None).expect("sources");

        assert!(best[0].url.path().ends_with("best.mp4"));
        assert!(smallest[0].url.path().ends_with("smallest.mp4"));
    }

    #[test]
    fn explicit_index_always_refers_to_resolve_output_order() {
        let post = video_post();
        for prefer in [Prefer::Best, Prefer::Smallest] {
            let selected = select_sources(&post, prefer, Some(1)).expect("source index 1");
            assert!(selected[0].url.path().ends_with("middle.mp4"));
        }
    }

    #[test]
    fn rejects_an_out_of_range_index() {
        let error = select_sources(&video_post(), Prefer::Best, Some(3)).expect_err("bad index");
        assert!(matches!(error, Error::Config(_)));
    }

    #[test]
    fn first_only_selects_the_first_image_but_does_not_change_video_fallbacks() {
        assert_eq!(
            requested_source_index(ContentKind::ImageSet, None, true),
            Some(0)
        );
        assert_eq!(requested_source_index(ContentKind::Video, None, true), None);
        assert_eq!(
            requested_source_index(ContentKind::ImageSet, Some(2), true),
            Some(2)
        );
    }
}
