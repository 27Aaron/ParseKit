//! CLI: thin wrapper around parse-kit.

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand, ValueEnum};
use parse_kit::{
    Error, MediaSource, MediaSourceKind, ParseKit, ParseKitBuilder, ResolvedPost, Result,
    VideoCodec, platforms::util::display_title,
};

const DEFAULT_MAX_BYTES: u64 = 200 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(
    name = "pk",
    version,
    about = "ParseKit CLI — resolve and download social media posts"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Prefer {
    /// Highest quality first (default).
    Best,
    /// Smallest / lowest quality first.
    Smallest,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Resolve share text or URL (print summary / JSON).
    Resolve {
        /// Share text or URL.
        input: String,
        /// Print full JSON instead of a short summary.
        #[arg(long)]
        json: bool,
    },
    /// Resolve and download a video source.
    Download {
        /// Share text or URL.
        input: String,
        /// Output directory (default: PARSE_KIT_OUTPUT_DIR or ./downloads).
        #[arg(short, long, env = "PARSE_KIT_OUTPUT_DIR")]
        output: Option<PathBuf>,
        /// Max bytes (default: PARSE_KIT_MAX_BYTES or 200 MiB).
        #[arg(long, env = "PARSE_KIT_MAX_BYTES")]
        max_bytes: Option<u64>,
        /// Which source to try first: best (default) or smallest.
        #[arg(long, value_enum, default_value_t = Prefer::Best)]
        prefer: Prefer,
        /// Download only this source index from `pk resolve` (0-based).
        #[arg(long)]
        source: Option<usize>,
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// List registered platforms for this build.
    Platforms,
}

fn main() -> ExitCode {
    load_dotenv();
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(error) => {
            eprintln!("error: failed to start runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    let result = runtime.block_on(async move {
        match cli.command {
            Commands::Resolve { input, json } => cmd_resolve(&input, json).await,
            Commands::Download {
                input,
                output,
                max_bytes,
                prefer,
                source,
                json,
            } => cmd_download(&input, output, max_bytes, prefer, source, json).await,
            Commands::Platforms => {
                cmd_platforms();
                Ok(())
            }
        }
    });

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn load_dotenv() {
    let _ = dotenvy::from_filename(".env.local");
    let _ = dotenvy::dotenv();
}

fn build_kit() -> Result<ParseKit> {
    let mut builder = ParseKitBuilder::new();
    match std::env::var("YUANBAO_COOKIE") {
        Ok(cookie) if !cookie.trim().is_empty() => {
            builder = builder.wechat(cookie)?;
        }
        _ => {}
    }
    builder = builder.douyin()?;
    builder.build()
}

async fn cmd_resolve(input: &str, json: bool) -> Result<()> {
    let kit = build_kit()?;
    let post = kit.resolve_text(input).await?;
    if json {
        print_post_json(&post)?;
    } else {
        print_post_summary(&post);
    }
    Ok(())
}

async fn cmd_download(
    input: &str,
    output: Option<PathBuf>,
    max_bytes: Option<u64>,
    prefer: Prefer,
    source: Option<usize>,
    json: bool,
) -> Result<()> {
    let kit = build_kit()?;
    let post = kit.resolve_text(input).await?;
    let sources = select_sources(&post, prefer, source)?;
    let dir = output.unwrap_or_else(default_output_dir);
    let limit = max_bytes.unwrap_or_else(default_max_bytes);
    let downloader = kit.media_downloader_for(&post, &dir, limit)?;
    let media = downloader
        .download_playable(sources.iter().copied())
        .await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "platform": post.platform,
                "post_id": post.post_id,
                "title": display_title(&post),
                "path": media.path,
                "bytes": media.bytes,
                "source_count": post.media_sources().count(),
            })
        );
    } else {
        print_post_summary(&post);
        println!("saved: {} ({} bytes)", media.path.display(), media.bytes);
    }

    // Keep file for the user; disable Drop cleanup by forgetting RAII.
    std::mem::forget(media);
    Ok(())
}

fn select_sources(
    post: &ResolvedPost,
    prefer: Prefer,
    source: Option<usize>,
) -> Result<Vec<&MediaSource>> {
    let mut sources: Vec<&MediaSource> = post.media_sources().collect();
    if sources.is_empty() {
        return Err(Error::MediaUnavailable);
    }

    match prefer {
        Prefer::Best => {}
        Prefer::Smallest => sources.reverse(),
    }

    if let Some(index) = source {
        let chosen = sources.get(index).copied().ok_or_else(|| {
            Error::Config(format!(
                "无效的 --source {index}（可用 0..{}）",
                sources.len().saturating_sub(1)
            ))
        })?;
        return Ok(vec![chosen]);
    }

    Ok(sources)
}

fn cmd_platforms() {
    println!("wechat_channels\t微信视频号\tneeds YUANBAO_COOKIE");
    println!("douyin\t抖音\tpublic share page");
}

fn print_post_summary(post: &ResolvedPost) {
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

fn print_post_json(post: &ResolvedPost) -> Result<()> {
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

fn default_output_dir() -> PathBuf {
    std::env::var_os("PARSE_KIT_OUTPUT_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("./downloads"))
}

fn default_max_bytes() -> u64 {
    std::env::var("PARSE_KIT_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_MAX_BYTES)
}
