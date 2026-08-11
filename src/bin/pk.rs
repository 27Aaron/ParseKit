//! CLI: thin wrapper around parse-kit.

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use parse_kit::{
    Error, ParseKit, ParseKitBuilder, ResolvedPost, Result, platforms::util::display_title,
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
    /// Resolve and download the primary video.
    Download {
        /// Share text or URL.
        input: String,
        /// Output directory (default: PARSE_KIT_OUTPUT_DIR or ./downloads).
        #[arg(short, long, env = "PARSE_KIT_OUTPUT_DIR")]
        output: Option<PathBuf>,
        /// Max bytes (default: PARSE_KIT_MAX_BYTES or 200 MiB).
        #[arg(long, env = "PARSE_KIT_MAX_BYTES")]
        max_bytes: Option<u64>,
        /// Keep temp file path printed; always writes under --output.
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
                json,
            } => cmd_download(&input, output, max_bytes, json).await,
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
    json: bool,
) -> Result<()> {
    let kit = build_kit()?;
    let post = kit.resolve_text(input).await?;
    let dir = output.unwrap_or_else(default_output_dir);
    let limit = max_bytes.unwrap_or_else(default_max_bytes);
    let downloader = kit.media_downloader_for(&post, &dir, limit)?;
    let media = downloader.download_playable(post.media_sources()).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "platform": post.platform,
                "post_id": post.post_id,
                "title": display_title(&post),
                "path": media.path,
                "bytes": media.bytes,
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
    println!("video:     {}", post.video.url);
    if !post.fallback_videos.is_empty() {
        println!("fallbacks: {}", post.fallback_videos.len());
    }
}

fn print_post_json(post: &ResolvedPost) -> Result<()> {
    let value = serde_json::json!({
        "platform": post.platform,
        "post_id": post.post_id,
        "title": display_title(post),
        "kind": post.kind,
        "canonical_url": post.canonical_url.as_str(),
        "cover_url": post.cover_url.as_ref().map(|u| u.as_str().to_owned()),
        "video": {
            "url": post.video.url.as_str(),
            "codec": post.video.codec,
            "width": post.video.width,
            "height": post.video.height,
            "has_decode_key": post.video.decode_key.is_some(),
        },
        "fallback_count": post.fallback_videos.len(),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|e| Error::Config(e.to_string()))?
    );
    Ok(())
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
