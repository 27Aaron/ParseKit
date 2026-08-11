//! CLI subcommand implementations.

use std::path::PathBuf;

use parse_kit::{ContentKind, Result, wechat::WechatCredentialStatus};

use crate::{
    args::Prefer,
    config::{self, build_kit},
    output::{print_post_json, print_post_summary},
    sources::{requested_source_index, select_sources},
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
    prefer: Prefer,
    source: Option<usize>,
    first_only: bool,
    json: bool,
) -> Result<()> {
    let kit = build_kit()?;
    let post = kit.resolve_text(input).await?;
    let dir = output.unwrap_or_else(config::default_output_dir);
    let downloader = kit.media_downloader_for(&post, &dir)?;

    let multi_image = post.kind == ContentKind::ImageSet && source.is_none() && !first_only;

    if multi_image {
        let sources: Vec<_> = post.media_sources().collect();
        let media_list = downloader.download_all(sources).await?;
        let paths: Vec<_> = media_list
            .into_iter()
            .map(|media| {
                let bytes = media.bytes;
                let path = media.keep();
                (path, bytes)
            })
            .collect();

        if json {
            println!(
                "{}",
                serde_json::json!({
                    "platform": post.platform,
                    "post_id": post.post_id,
                    "title": post.display_title(),
                    "kind": post.kind,
                    "files": paths.iter().map(|(p, b)| serde_json::json!({
                        "path": p,
                        "bytes": b,
                    })).collect::<Vec<_>>(),
                })
            );
        } else {
            print_post_summary(&post);
            for (path, bytes) in &paths {
                println!("saved: {} ({} bytes)", path.display(), bytes);
            }
        }
        return Ok(());
    }

    let selected_index = requested_source_index(post.kind, source, first_only);
    let sources = select_sources(&post, prefer, selected_index)?;
    let media = downloader
        .download_playable(sources.iter().copied())
        .await?;
    let bytes = media.bytes;
    let path = media.keep();

    if json {
        println!(
            "{}",
            serde_json::json!({
                "platform": post.platform,
                "post_id": post.post_id,
                "title": post.display_title(),
                "path": path,
                "bytes": bytes,
                "source_count": post.media_sources().count(),
            })
        );
    } else {
        print_post_summary(&post);
        println!("saved: {} ({} bytes)", path.display(), bytes);
    }
    Ok(())
}

pub fn platforms(check: bool) -> Result<()> {
    let kit = build_kit()?;
    for platform in kit.platforms() {
        println!(
            "{}\t{}\t{}",
            platform.platform_id(),
            platform.display_name(),
            platform.capability_note()
        );
    }
    if kit.wechat().is_none() {
        eprintln!("note: set YUANBAO_COOKIE to enable wechat");
    }
    if check {
        print_health(&kit);
    }
    Ok(())
}

pub fn doctor() -> Result<()> {
    let kit = build_kit()?;
    println!("parse-kit doctor");
    println!("platforms: {}", kit.platforms().len());
    for platform in kit.platforms() {
        println!(
            "  - {} ({})",
            platform.platform_id(),
            platform.display_name()
        );
    }
    print_health(&kit);
    Ok(())
}

fn print_health(kit: &parse_kit::ParseKit) {
    match kit.wechat() {
        None => println!("wechat: disabled (no YUANBAO_COOKIE)"),
        Some(wechat) => match wechat.credential_status() {
            WechatCredentialStatus::Present => {
                println!("wechat: cookie present (shape ok; not network-verified)")
            }
            WechatCredentialStatus::Incomplete => {
                println!("wechat: cookie incomplete (missing hy_user/token markers)")
            }
        },
    }
    println!(
        "douyin: {}",
        if kit.douyin().is_some() {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!(
        "bilibili: {}",
        if kit.bilibili().is_some() {
            "enabled"
        } else {
            "disabled"
        }
    );
}
