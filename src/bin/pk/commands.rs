//! CLI subcommand implementations.

use std::{path::PathBuf, time::Duration};

use parse_kit::{
    ContentKind, CredentialStatus, MediaSource, ResolvedPost, Result, media::MediaDownloader,
    wechat::WechatCredentialStatus,
};

use crate::{
    args::Prefer,
    config::{self, build_kit},
    output::{print_post_json, print_post_summary, stream_post_header, stream_post_summary},
    select,
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

    if human {
        stream_post_header(&post).await;
    }

    let dir = output.unwrap_or_else(config::default_output_dir);
    let mut downloader = kit.media_downloader_for(&post, &dir)?;
    if force {
        downloader = downloader.with_skip_existing(false);
    }

    let all_sources: Vec<&MediaSource> = post.media_sources().collect();
    let multi_image = post.kind == ContentKind::ImageSet && source.is_none() && !first_only;

    // Interactive pick when TTY and user did not pin --source / first_only.
    let chosen: Vec<&MediaSource> =
        if human && source.is_none() && !first_only && all_sources.len() > 1 {
            let table = select::source_option_table(&all_sources);
            let header = Some(table.header.as_str());
            let picked = if multi_image {
                let indices = select::pick_many(&table.rows, true, header)?;
                match indices {
                    Some(idxs) => idxs,
                    None => {
                        ui::err("Cancelled", "未选择任何源");
                        return Ok(());
                    }
                }
            } else {
                let default = match prefer {
                    Prefer::Best => 0,
                    Prefer::Smallest => all_sources.len().saturating_sub(1),
                };
                match select::pick_one(&table.rows, default, header)? {
                    Some(idx) => vec![idx],
                    None => {
                        ui::err("Cancelled", "未选择任何源");
                        return Ok(());
                    }
                }
            };
            ui::ok(
                "Selected",
                format!(
                    "{} 路  ·  {}",
                    picked.len(),
                    picked
                        .iter()
                        .map(|i| format!("[{}]", i + 1))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
            );
            picked.into_iter().map(|i| all_sources[i]).collect()
        } else if multi_image {
            all_sources
        } else {
            let selected_index = requested_source_index(post.kind, source, first_only);
            select_sources(&post, prefer, selected_index)?
        };

    if chosen.is_empty() {
        return Err(parse_kit::Error::MediaUnavailable);
    }

    // Multi-file download (image set or multi-select).
    if chosen.len() > 1 {
        return download_many(human, &post, &downloader, &chosen, json).await;
    }

    // Single source (video or one image).
    let sources = chosen;
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

async fn download_many(
    human: bool,
    post: &ResolvedPost,
    downloader: &MediaDownloader,
    sources: &[&MediaSource],
    json: bool,
) -> Result<()> {
    let total_files = sources.len();
    let download_spin = if human {
        Some(Spinner::start(format!("Downloading  1/{total_files}…")))
    } else {
        None
    };

    let mut paths: Vec<(PathBuf, u64, bool)> = Vec::with_capacity(total_files);
    let mut skipped = 0usize;
    for (index, media_source) in sources.iter().enumerate() {
        let file_no = index + 1;
        // Sequence for multi-file naming must match original post index when possible.
        // download() always uses sequence 0 unless download_all; use download_with_progress
        // and rely on stem + sequence from downloader API.
        // MediaDownloader::download uses sequence 0 always — for multi we need sequence.
        // Use download_with_progress via private sequence? Only download_all uses sequence.
        // For selected subset, sequence index of pick order is fine for unique names.
        let sequence = u32::try_from(index).unwrap_or(u32::MAX);
        let media = if let Some(spinner) = download_spin.as_ref() {
            let label = spinner.label();
            label.set(format!("Downloading  {file_no}/{total_files}…"));
            let label_prefix = format!("Downloading  {file_no}/{total_files}");
            match downloader
                .download_indexed_with_progress(media_source, sequence, move |progress| {
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
            match downloader.download_indexed(media_source, sequence).await {
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
        print_post_summary(post);
        for (path, bytes, was_skipped) in &paths {
            let verb = if *was_skipped { "exists" } else { "saved" };
            println!("{verb}: {} ({} bytes)", path.display(), bytes);
        }
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
    if kit
        .bilibili()
        .is_some_and(|b| b.credential_status() == CredentialStatus::Absent)
    {
        ui::note("optional: BILIBILI_COOKIE or `pk bilibili login` for higher quality");
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
    match kit.bilibili() {
        None => ui::note("bilibili disabled"),
        Some(bilibili) => match bilibili.credential_status() {
            CredentialStatus::Present => {
                ui::ok(
                    "bilibili",
                    "cookie present (shape ok; not network-verified)",
                );
            }
            CredentialStatus::Incomplete => {
                ui::err("bilibili", "cookie incomplete (missing SESSDATA)");
            }
            CredentialStatus::Absent => {
                ui::note("bilibili anonymous (set BILIBILI_COOKIE or run: pk bilibili login)");
            }
        },
    }
}

/// Web QR login → write `BILIBILI_COOKIE` into `.env.local`.
pub async fn bilibili_login() -> Result<()> {
    ui::note("requesting Bilibili QR login…");
    let session = parse_kit::bilibili::start_web_qr_login().await?;

    print_qr_or_url(&session.url);

    ui::note("open the Bilibili app → scan → confirm");
    ui::note("waiting for scan (timeout 180s)…");

    let cookie = parse_kit::bilibili::wait_web_qr_login(
        &session.qrcode_key,
        Duration::from_secs(1),
        Duration::from_secs(180),
    )
    .await?;

    config::save_bilibili_cookie(cookie.as_str())?;
    let path = config::env_local_path();
    ui::ok(
        "login",
        format!("saved BILIBILI_COOKIE to {}", path.display()),
    );
    ui::note("reload shell env if needed; next `pk` in this dir loads .env.local automatically");
    Ok(())
}

pub fn bilibili_logout() -> Result<()> {
    let removed = config::clear_bilibili_cookie()?;
    if removed {
        ui::ok("logout", "removed BILIBILI_COOKIE from .env.local");
    } else {
        ui::note("no BILIBILI_COOKIE line in .env.local (process env cleared if set)");
    }
    Ok(())
}

pub fn bilibili_status() -> Result<()> {
    let kit = build_kit()?;
    match kit.bilibili() {
        None => ui::note("bilibili not registered"),
        Some(bilibili) => match bilibili.credential_status() {
            CredentialStatus::Present => {
                ui::ok(
                    "bilibili",
                    "logged in (SESSDATA present; not network-verified)",
                );
            }
            CredentialStatus::Incomplete => {
                ui::err("bilibili", "BILIBILI_COOKIE set but missing SESSDATA");
            }
            CredentialStatus::Absent => {
                ui::note("bilibili anonymous — paste cookie into .env or run: pk bilibili login");
            }
        },
    }
    Ok(())
}

fn print_qr_or_url(url: &str) {
    match render_terminal_qr(url) {
        Ok(art) => {
            println!();
            println!("{art}");
            println!();
            ui::note(format!("if the QR is unreadable, open: {url}"));
        }
        Err(_) => {
            ui::note(format!("open this URL (or encode as QR): {url}"));
        }
    }
}

fn render_terminal_qr(url: &str) -> std::result::Result<String, ()> {
    use qrcode::QrCode;
    use qrcode::render::unicode::Dense1x2;

    let code = QrCode::new(url.as_bytes()).map_err(|_| ())?;
    Ok(code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Dark)
        .light_color(Dense1x2::Light)
        .build())
}
