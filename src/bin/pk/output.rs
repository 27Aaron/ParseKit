//! Human and JSON output.

use parse_kit::{
    Error, MediaSource, MediaSourceKind, ResolvedPost, Result, VideoCodec,
    platforms::util::display_title,
};

pub fn print_post_summary(post: &ResolvedPost) {
    println!("platform:  {}", post.platform);
    println!("post_id:   {}", post.post_id);
    println!("title:     {}", display_title(post));
    println!("kind:      {:?}", post.kind);
    println!("canonical: {}", post.canonical_url);
    println!("sources:");
    for (index, source) in post.media_sources().enumerate() {
        let mark = if index == 0 { " *" } else { "" };
        println!(
            "  [{index}] {}{}  {}  {}  {}  decode_key={}",
            source_kind_label(source),
            mark,
            format_dims(source),
            format_size(source),
            format_codec(source.codec),
            if source.decode_key.is_some() {
                "yes"
            } else {
                "no"
            },
        );
        println!("       {}", source.url);
    }
    if post.media_sources().count() > 1 {
        println!("hint:     download --source N  or  --prefer smallest");
    }
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
        "title": display_title(post),
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
        Some(bytes) if bytes >= 1024 * 1024 => {
            format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
        }
        Some(bytes) if bytes >= 1024 => format!("{}KB", bytes / 1024),
        Some(bytes) => format!("{bytes}B"),
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
