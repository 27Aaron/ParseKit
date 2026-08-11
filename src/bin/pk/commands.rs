//! CLI subcommand implementations.

use std::path::PathBuf;

use parse_kit::{ContentKind, Result, wechat::WechatCredentialStatus};

use crate::{
    args::Prefer,
    config::{self, build_kit},
    output::{print_post_json, print_post_summary, stream_post_summary},
    sources::{requested_source_index, select_sources},
    ui::{self, Spinner},
};

pub async fn resolve(input: &str, json: bool) -> Result<()> {
    let kit = build_kit()?;
    let spinner = if ui::interactive(json) {
        Some(Spinner::start("Resolving…"))
    } else {
        None
    };

    let post = match kit.resolve_text(input).await {
        Ok(post) => post,
        Err(error) => {
            if let Some(spinner) = spinner {
                spinner.finish_err("Failed", "resolve").await;
            }
            return Err(error);
        }
    };

    if let Some(spinner) = spinner {
        spinner.finish_silent().await;
    }

    if json {
        print_post_json(&post)?;
    } else if ui::interactive(json) {
        stream_post_summary(&post).await;
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
    force: bool,
    json: bool,
) -> Result<()> {
    let kit = build_kit()?;
    let human = ui::interactive(json);

    let resolve_spin = if human {
        Some(Spinner::start("Resolving…"))
    } else {
        None
    };
    let post = match kit.resolve_text(input).await {
        Ok(post) => post,
        Err(error) => {
            if let Some(spinner) = resolve_spin {
                spinner.finish_err("Failed", "resolve").await;
            }
            return Err(error);
        }
    };
    if let Some(spinner) = resolve_spin {
        spinner.finish_silent().await;
    }

    let dir = output.unwrap_or_else(config::default_output_dir);
    let mut downloader = kit.media_downloader_for(&post, &dir)?;
    if force {
        downloader = downloader.with_skip_existing(false);
    }
    let multi_image = post.kind == ContentKind::ImageSet && source.is_none() && !first_only;

    if multi_image {
        let sources: Vec<_> = post.media_sources().collect();
        let total_files = sources.len();
        let download_spin = if human {
            Some(Spinner::start(format!(
                "Downloading  1/{total_files} images…"
            )))
        } else {
            None
        };

        let mut paths: Vec<(PathBuf, u64, bool)> = Vec::with_capacity(total_files);
        let mut skipped = 0usize;
        for (index, media_source) in sources.into_iter().enumerate() {
            let file_no = index + 1;
            let media = if let Some(spinner) = download_spin.as_ref() {
                let label = spinner.label();
                label.set(format!("Downloading  {file_no}/{total_files} images…"));
                let label_prefix = format!("Downloading  {file_no}/{total_files}");
                match downloader
                    .download_with_progress(media_source, move |progress| {
                        label.set(format!(
                            "{label_prefix}  {:>3}%  ·  {} / {}",
                            progress.percent,
                            ui::format_bytes(progress.downloaded_bytes),
                            ui::format_bytes(progress.total_bytes)
                        ));
                    })
                    .await
                {
                    Ok(media) => media,
                    Err(error) => {
                        if let Some(spinner) = download_spin {
                            spinner.finish_err("Failed", "download").await;
                        }
                        return Err(error);
                    }
                }
            } else {
                match downloader.download(media_source).await {
                    Ok(media) => media,
                    Err(error) => {
                        for (path, _, was_skipped) in paths.drain(..) {
                            if !was_skipped {
                                let _ = tokio::fs::remove_file(path).await;
                            }
                        }
                        return Err(error);
                    }
                }
            };
            let was_skipped = media.skipped;
            if was_skipped {
                skipped += 1;
            }
            let bytes = media.bytes;
            let path = media.keep();
            paths.push((path, bytes, was_skipped));
        }

        if let Some(spinner) = download_spin {
            let total: u64 = paths.iter().map(|(_, b, _)| *b).sum();
            let (action, detail) = if skipped == paths.len() {
                (
                    "Already saved",
                    format!("{} files  ·  {}", paths.len(), ui::format_bytes(total)),
                )
            } else if skipped > 0 {
                (
                    "Saved",
                    format!(
                        "{} files ({} present)  ·  {}",
                        paths.len(),
                        skipped,
                        ui::format_bytes(total)
                    ),
                )
            } else {
                (
                    "Saved",
                    format!("{} files  ·  {}", paths.len(), ui::format_bytes(total)),
                )
            };
            spinner.finish_ok(action, detail).await;
        }

        if json {
            println!(
                "{}",
                serde_json::json!({
                    "platform": post.platform,
                    "post_id": post.post_id,
                    "title": post.display_title(),
                    "kind": post.kind,
                    "files": paths.iter().map(|(p, b, s)| serde_json::json!({
                        "path": p,
                        "bytes": b,
                        "skipped": s,
                    })).collect::<Vec<_>>(),
                })
            );
        } else if !human {
            print_post_summary(&post);
            for (path, bytes, was_skipped) in &paths {
                let verb = if *was_skipped { "exists" } else { "saved" };
                println!("{verb}: {} ({} bytes)", path.display(), bytes);
            }
        }
        return Ok(());
    }

    let selected_index = requested_source_index(post.kind, source, first_only);
    let sources = select_sources(&post, prefer, selected_index)?;
    let download_spin = if human {
        Some(Spinner::start("Downloading…"))
    } else {
        None
    };
    let media = if let Some(spinner) = download_spin.as_ref() {
        let label = spinner.label();
        match downloader
            .download_playable_with_progress(sources.iter().copied(), move |progress| {
                label.set(ui::download_progress_label(
                    progress.percent,
                    progress.downloaded_bytes,
                    progress.total_bytes,
                ));
            })
            .await
        {
            Ok(media) => media,
            Err(error) => {
                if let Some(spinner) = download_spin {
                    spinner.finish_err("Failed", "download").await;
                }
                return Err(error);
            }
        }
    } else {
        downloader
            .download_playable(sources.iter().copied())
            .await?
    };
    let skipped = media.skipped;
    let bytes = media.bytes;
    let path = media.keep();

    if let Some(spinner) = download_spin {
        let action = if skipped { "Already saved" } else { "Saved" };
        spinner
            .finish_ok(
                action,
                format!("{}  ·  {}", path.display(), ui::format_bytes(bytes)),
            )
            .await;
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "platform": post.platform,
                "post_id": post.post_id,
                "title": post.display_title(),
                "path": path,
                "bytes": bytes,
                "skipped": skipped,
                "source_count": post.media_sources().count(),
            })
        );
    } else if !human {
        print_post_summary(&post);
        let verb = if skipped { "exists" } else { "saved" };
        println!("{verb}: {} ({} bytes)", path.display(), bytes);
    }
    Ok(())
}

pub fn platforms(check: bool) -> Result<()> {
    let kit = build_kit()?;
    for platform in kit.platforms() {
        ui::platform_row(
            platform.platform_id().as_str(),
            platform.display_name(),
            platform.capability_note(),
        );
    }
    if kit.wechat().is_none() {
        ui::note("set YUANBAO_COOKIE to enable wechat");
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
        ui::platform_row(
            platform.platform_id().as_str(),
            platform.display_name(),
            "registered",
        );
    }
    print_health(&kit);
    Ok(())
}

fn print_health(kit: &parse_kit::ParseKit) {
    match kit.wechat() {
        None => ui::note("wechat disabled (no YUANBAO_COOKIE)"),
        Some(wechat) => match wechat.credential_status() {
            WechatCredentialStatus::Present => {
                ui::ok("wechat", "cookie present (shape ok; not network-verified)")
            }
            WechatCredentialStatus::Incomplete => ui::err(
                "wechat",
                "cookie incomplete (missing hy_user/token markers)",
            ),
        },
    }
    if kit.douyin().is_some() {
        ui::ok("douyin", "enabled");
    } else {
        ui::note("douyin disabled");
    }
    if kit.bilibili().is_some() {
        ui::ok("bilibili", "enabled");
    } else {
        ui::note("bilibili disabled");
    }
}
