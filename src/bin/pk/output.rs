//! Render human-readable and JSON output.

use parse_kit::{Error, MediaSource, ResolvedPost, Result};

use crate::ui;

/// Instant summary (non-TTY / scripts). Every field is a ✓ row.
pub fn print_post_summary(post: &ResolvedPost) {
    for (action, detail) in summary_rows(post) {
        ui::ok(&action, detail);
    }
    let total = post.media_sources().count();
    ui::ok("sources", format!("{total} 路（[0] 默认最高）"));
    for (index, source) in post.media_sources().enumerate() {
        let mark = if index == 0 { "★" } else { "·" };
        ui::ok(
            &format!("source[{index}]"),
            format!("{mark}  {}", source.quality_summary()),
        );
        ui::sub(source.url.as_str());
    }
    if total > 1 {
        ui::ok("hint", "download --source N  or  --prefer smallest");
    }
}

/// Streamed summary with spin→✓ reveal (interactive TTY).
pub async fn stream_post_summary(post: &ResolvedPost) {
    ui::reveal_ok_rows(summary_rows(post)).await;
    let total = post.media_sources().count();
    ui::reveal_ok("sources", format!("{total} 路（[0] 默认最高）")).await;
    for (index, source) in post.media_sources().enumerate() {
        let mark = if index == 0 { "★" } else { "·" };
        ui::reveal_ok(
            &format!("source[{index}]"),
            format!("{mark}  {}", source.quality_summary()),
        )
        .await;
        ui::reveal_sub(source.url.as_str()).await;
    }
    if total > 1 {
        ui::reveal_ok("hint", "download --source N  or  --prefer smallest").await;
    }
}

fn summary_rows(post: &ResolvedPost) -> Vec<(String, String)> {
    vec![
        ("platform".into(), post.platform.to_string()),
        ("post_id".into(), post.post_id.clone()),
        ("title".into(), ui::one_line(&post.display_title(), 100)),
        ("kind".into(), format!("{:?}", post.kind)),
        ("canonical".into(), post.canonical_url.as_str().to_owned()),
    ]
}

pub fn print_post_json(post: &ResolvedPost) -> Result<()> {
    let sources: Vec<_> = post
        .media_sources()
        .enumerate()
        .map(|(index, source)| source_json(index, source))
        .collect();

    let value = serde_json::json!({
        "platform": post.platform,
        "post_id": post.post_id,
        "title": post.display_title(),
        "kind": post.kind,
        "canonical_url": post.canonical_url.as_str(),
        "cover_url": post.cover_url.as_ref().map(|u| u.as_str().to_owned()),
        "sources": sources,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|e| Error::Config(e.to_string()))?
    );
    Ok(())
}

fn source_json(index: usize, source: &MediaSource) -> serde_json::Value {
    serde_json::json!({
        "index": index,
        "label": source.quality_label(),
        "summary": source.quality_summary(),
        "kind": match source.provenance {
            parse_kit::MediaSourceKind::Direct => "origin",
            parse_kit::MediaSourceKind::Derived => "derived",
            parse_kit::MediaSourceKind::H264 => "h264",
            parse_kit::MediaSourceKind::H265 => "h265",
            parse_kit::MediaSourceKind::Generic => "generic",
        },
        "codec": source.codec,
        "width": source.width,
        "height": source.height,
        "size_hint": source.size_hint,
        "bitrate_bps": source.bitrate_bps,
        "has_decode_key": source.decode_key.is_some(),
        "url": source.url.as_str(),
        "default": index == 0,
    })
}
