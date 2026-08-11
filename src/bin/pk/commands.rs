//! Subcommand handlers.

use std::path::PathBuf;

use parse_kit::{Result, platforms::util::display_title};

use crate::{
    args::Prefer,
    config::{self, build_kit},
    output::{print_post_json, print_post_summary},
    sources::select_sources,
};

pub async fn resolve(input: &str, json: bool) -> Result<()> {
    let kit = build_kit()?;
    let post = kit.resolve_text(input).await?;
    if json {
        print_post_json(&post)?;
    } else {
        print_post_summary(&post);
    }
    Ok(())
}

pub async fn download(
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
    let dir = output.unwrap_or_else(config::default_output_dir);
    let limit = max_bytes.unwrap_or_else(config::default_max_bytes);
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

    // Keep file for the user; Drop would delete it.
    std::mem::forget(media);
    Ok(())
}

pub fn platforms() {
    println!("wechat_channels\t微信视频号\tneeds YUANBAO_COOKIE");
    println!("douyin\t抖音\tpublic share page");
}
