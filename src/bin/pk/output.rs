//! Render human-readable and JSON output.

use parse_kit::{Error, MediaSource, MediaSourceKind, ResolvedPost, Result, VideoCodec};

use crate::ui;

const LABEL_WIDTH: usize = 10;

pub fn print_post_summary(post: &ResolvedPost) {
    kv("platform", post.platform.to_string());
    kv("post_id", &post.post_id);
    kv("title", ui::one_line(&post.display_title(), 100));
    kv("kind", format!("{:?}", post.kind));
    kv("canonical", post.canonical_url.as_str());

    println!("  {:LABEL_WIDTH$}", "sources");
    for (index, source) in post.media_sources().enumerate() {
        let mark = if index == 0 { "*" } else { " " };
        println!(
            "  {:LABEL_WIDTH$}  [{index}] {mark}  {:<8}  {:>10}  {:>8}  {:<8}  key={}",
            "",
            source_kind_label(source),
            format_dims(source),
            format_size(source),
            format_codec(source.codec),
            if source.decode_key.is_some() {
                "yes"
            } else {
                "no"
            },
        );
        println!("  {:LABEL_WIDTH$}      {}", "", source.url);
    }
    if post.media_sources().count() > 1 {
        kv("hint", "download --source N  or  --prefer smallest");
    }
}

fn kv(label: &str, value: impl AsRef<str>) {
    println!("  {label:<LABEL_WIDTH$}  {}", value.as_ref());
}

pub fn print_post_json(post: &ResolvedPost) -> Result<()> {
    let sources: Vec<_> = post
        .media_sources()
        .enumerate()
        .map(|(index, source)| {
            serde_json::json!({
                "index": index,
                "kind": source_kind_label(source),
                "codec": source.codec,
                "width": source.width,
                "height": source.height,
                "size_hint": source.size_hint,
                "has_decode_key": source.decode_key.is_some(),
                "url": source.url.as_str(),
                "default": index == 0,
            })
        })
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

fn source_kind_label(source: &MediaSource) -> &'static str {
    match source.provenance {
        MediaSourceKind::Direct => "origin",
        MediaSourceKind::Derived => "derived",
        MediaSourceKind::H264 => "h264",
        MediaSourceKind::H265 => "h265",
        MediaSourceKind::Generic => "generic",
    }
}

fn format_dims(source: &MediaSource) -> String {
    match (source.width, source.height) {
        (Some(w), Some(h)) => format!("{w}x{h}"),
        _ => "-".into(),
    }
}

fn format_size(source: &MediaSource) -> String {
    match source.size_hint {
        Some(bytes) => ui::format_bytes(bytes),
        None => "-".into(),
    }
}

fn format_codec(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "h264",
        VideoCodec::H265 => "h265",
        VideoCodec::Unknown => "unknown",
    }
}
