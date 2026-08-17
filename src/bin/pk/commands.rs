//! CLI subcommand implementations.

use std::{path::PathBuf, time::Duration};

use parse_kit::{
    ContentKind, CredentialStatus, MediaSource, ResolvedPost, Result,
    media::{DownloadedMedia, MediaDownloader},
    wechat::{self, WechatCredentialStatus},
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

    // Prompt only when the source is not pinned.
    let chosen: Vec<(usize, &MediaSource)> =
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
            picked
                .into_iter()
                .map(|index| (index, all_sources[index]))
                .collect()
        } else if multi_image {
            all_sources.into_iter().enumerate().collect()
        } else {
            let selected_index = requested_source_index(post.kind, source, first_only);
            select_sources(&post, prefer, selected_index)?
        };

    if chosen.is_empty() {
        return Err(parse_kit::Error::MediaUnavailable);
    }

    // Images are independent; video variants are fallbacks.
    let strategy = download_strategy(post.kind, chosen.len(), chosen[0].0)?;
    if strategy == DownloadStrategy::IndependentSet {
        return download_many(human, &post, &downloader, &chosen, json).await;
    }

    let sources = chosen;
    let download_spin = if human {
        Some(Spinner::start("Downloading…"))
    } else {
        None
    };
    let media = if let Some(spinner) = download_spin.as_ref() {
        let label = spinner.label();
        let on_progress = move |progress: parse_kit::media::DownloadProgress| {
            label.set(ui::download_progress_label(
                progress.percent,
                progress.downloaded_bytes,
                progress.total_bytes,
            ));
        };
        let result = match strategy {
            DownloadStrategy::IndexedSingle(sequence) => {
                downloader
                    .download_indexed_with_progress(sources[0].1, sequence, on_progress)
                    .await
            }
            DownloadStrategy::PlayableFallback => {
                downloader
                    .download_playable_with_progress(
                        sources.iter().map(|(_, source)| *source),
                        on_progress,
                    )
                    .await
            }
            DownloadStrategy::IndependentSet => unreachable!("handled above"),
        };
        match result {
            Ok(media) => media,
            Err(error) => {
                if let Some(spinner) = download_spin {
                    spinner.finish_err("Failed", "download").await;
                }
                return Err(error);
            }
        }
    } else {
        match strategy {
            DownloadStrategy::IndexedSingle(sequence) => {
                downloader.download_indexed(sources[0].1, sequence).await?
            }
            DownloadStrategy::PlayableFallback => {
                downloader
                    .download_playable(sources.iter().map(|(_, source)| *source))
                    .await?
            }
            DownloadStrategy::IndependentSet => unreachable!("handled above"),
        }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DownloadStrategy {
    IndependentSet,
    IndexedSingle(u32),
    PlayableFallback,
}

fn download_strategy(
    kind: ContentKind,
    source_count: usize,
    source_index: usize,
) -> Result<DownloadStrategy> {
    if kind != ContentKind::ImageSet {
        return Ok(DownloadStrategy::PlayableFallback);
    }
    if source_count > 1 {
        return Ok(DownloadStrategy::IndependentSet);
    }
    let sequence = u32::try_from(source_index)
        .map_err(|_| parse_kit::Error::Config("媒体源序号超出支持范围".to_owned()))?;
    Ok(DownloadStrategy::IndexedSingle(sequence))
}

async fn download_many(
    human: bool,
    post: &ResolvedPost,
    downloader: &MediaDownloader,
    sources: &[(usize, &MediaSource)],
    json: bool,
) -> Result<()> {
    let total_files = sources.len();
    let download_spin = if human {
        Some(Spinner::start(format!("Downloading  1/{total_files}…")))
    } else {
        None
    };

    let mut downloads = Vec::with_capacity(total_files);
    let mut skipped = 0usize;
    for (download_index, &(source_index, media_source)) in sources.iter().enumerate() {
        let file_no = download_index + 1;
        let sequence = u32::try_from(source_index)
            .map_err(|_| parse_kit::Error::Config("媒体源数量超出支持范围".into()))?;
        let media_result = if let Some(spinner) = download_spin.as_ref() {
            let label = spinner.label();
            label.set(format!("Downloading  {file_no}/{total_files}…"));
            let label_prefix = format!("Downloading  {file_no}/{total_files}");
            downloader
                .download_indexed_with_progress(media_source, sequence, move |progress| {
                    label.set(format!(
                        "{label_prefix}  {:>3}%  ·  {} / {}",
                        progress.percent,
                        ui::format_bytes(progress.downloaded_bytes),
                        ui::format_bytes(progress.total_bytes)
                    ));
                })
                .await
        } else {
            downloader.download_indexed(media_source, sequence).await
        };
        let media = match media_result {
            Ok(media) => media,
            Err(error) => {
                if let Some(spinner) = download_spin {
                    spinner.finish_err("Failed", "download").await;
                }
                cleanup_new_downloads(&mut downloads).await;
                return Err(error);
            }
        };
        if media.skipped {
            skipped += 1;
        }
        downloads.push(media);
    }

    let paths: Vec<(PathBuf, u64, bool)> = downloads
        .into_iter()
        .map(|media| {
            let bytes = media.bytes;
            let was_skipped = media.skipped;
            (media.keep(), bytes, was_skipped)
        })
        .collect();

    if let Some(spinner) = download_spin {
        let total = paths
            .iter()
            .fold(0_u64, |total, (_, bytes, _)| total.saturating_add(*bytes));
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

async fn cleanup_new_downloads(downloads: &mut Vec<DownloadedMedia>) {
    for media in downloads.drain(..) {
        if let Err(error) = media.cleanup().await {
            tracing::warn!(
                path = %media.path.display(),
                %error,
                "failed to clean up a partial multi-file download"
            );
        }
    }
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
        ui::note("set YUANBAO_COOKIE or run: pk wechat login");
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
        None => ui::note("wechat disabled (no YUANBAO_COOKIE; run: pk wechat login)"),
        Some(wechat) => match wechat.credential_status() {
            WechatCredentialStatus::Present => {
                ui::ok("wechat", "cookie present (shape ok; not network-verified)");
            }
            WechatCredentialStatus::Incomplete => {
                ui::err(
                    "wechat",
                    "cookie incomplete (missing hy_user/token markers)",
                );
            }
            WechatCredentialStatus::Absent => {
                ui::note("wechat cookie empty");
            }
        },
    }
    if kit.douyin().is_some() {
        ui::note("douyin temporarily unavailable (browser verification required)");
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

/// Runs WeChat QR login and saves `YUANBAO_COOKIE`.
pub async fn wechat_login() -> Result<()> {
    ui::note("requesting Yuanbao WeChat QR login…");
    let session = wechat::start_web_qr_login().await?;
    print_yuanbao_qr(&session);

    ui::note("open WeChat → scan → confirm");
    ui::note("waiting for scan (timeout 180s)…");
    let cookie =
        wechat::wait_web_qr_login(&session, Duration::from_secs(1), Duration::from_secs(180))
            .await?;
    config::save_yuanbao_cookie(cookie.as_str())?;
    let path = config::env_local_path();
    ui::ok(
        "login",
        format!("saved YUANBAO_COOKIE to {}", path.display()),
    );
    ui::note("next `pk` in this dir loads .env.local automatically");
    Ok(())
}

pub fn wechat_logout() -> Result<()> {
    let removed = config::clear_yuanbao_cookie()?;
    if removed {
        ui::ok("logout", "removed YUANBAO_COOKIE from .env.local");
    } else {
        ui::note("no YUANBAO_COOKIE entry in .env.local");
    }
    Ok(())
}

pub fn wechat_status() -> Result<()> {
    let kit = build_kit()?;
    match kit.wechat() {
        None => {
            ui::note("wechat disabled — set YUANBAO_COOKIE or run: pk wechat login");
        }
        Some(wechat) => match wechat.credential_status() {
            CredentialStatus::Present => {
                ui::ok(
                    "wechat",
                    "logged in (hy_user/token present; not network-verified)",
                );
            }
            CredentialStatus::Incomplete => {
                ui::err(
                    "wechat",
                    "YUANBAO_COOKIE set but missing hy_user/token markers",
                );
            }
            CredentialStatus::Absent => {
                ui::note("wechat cookie empty");
            }
        },
    }
    Ok(())
}

/// Runs Bilibili QR login and saves `BILIBILI_COOKIE`.
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
        ui::note("no BILIBILI_COOKIE entry in .env.local");
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
                ui::note("bilibili anonymous — set BILIBILI_COOKIE or run: pk bilibili login");
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

fn print_yuanbao_qr(session: &wechat::QrLoginSession) {
    match render_terminal_qr_image(session.qrcode_image()) {
        Ok(art) => {
            println!();
            println!("{art}");
            println!();
        }
        Err(_) => {
            ui::note(format!(
                "terminal cannot render the QR image; open: {}",
                session.qrcode_url()
            ));
        }
    }
}

fn render_terminal_qr_image(image: &[u8]) -> std::result::Result<String, ()> {
    use jpeg_decoder::{Decoder, PixelFormat};
    use std::io::Cursor;

    const MAX_QR_IMAGE_DIMENSION: usize = 4_096;

    let mut decoder = Decoder::new(Cursor::new(image));
    decoder.read_info().map_err(|_| ())?;
    let info = decoder.info().ok_or(())?;
    let width = usize::from(info.width);
    let height = usize::from(info.height);
    if width == 0
        || height == 0
        || width > MAX_QR_IMAGE_DIMENSION
        || height > MAX_QR_IMAGE_DIMENSION
    {
        return Err(());
    }
    let mut pixels = decoder.decode().map_err(|_| ())?;
    let luma = match info.pixel_format {
        PixelFormat::L8 => pixels,
        PixelFormat::RGB24 => {
            let pixel_count = width.checked_mul(height).ok_or(())?;
            if pixels.len() != pixel_count.checked_mul(3).ok_or(())? {
                return Err(());
            }
            for index in 0..pixel_count {
                let offset = index * 3;
                let value = u32::from(pixels[offset]) * 299
                    + u32::from(pixels[offset + 1]) * 587
                    + u32::from(pixels[offset + 2]) * 114;
                pixels[index] =
                    u8::try_from(value / 1000).expect("RGB-to-luma conversion stays within u8");
            }
            pixels.truncate(pixel_count);
            pixels
        }
        _ => return Err(()),
    };
    render_terminal_qr_luma(&luma, width, height)
}

fn render_terminal_qr_luma(
    pixels: &[u8],
    width: usize,
    height: usize,
) -> std::result::Result<String, ()> {
    const DARK_THRESHOLD: u8 = 128;

    if width == 0 || width != height || pixels.len() != width.checked_mul(height).ok_or(())? {
        return Err(());
    }

    let mut min_x = width;
    let mut min_y = height;
    for (index, value) in pixels.iter().enumerate() {
        if *value < DARK_THRESHOLD {
            min_x = min_x.min(index % width);
            min_y = min_y.min(index / width);
        }
    }
    if min_x == width || min_y == height {
        return Err(());
    }

    let row = &pixels[min_y * width..(min_y + 1) * width];
    let mut runs = Vec::new();
    let mut current_dark = row[0] < DARK_THRESHOLD;
    let mut current_len = 1usize;
    for value in &row[1..] {
        let dark = *value < DARK_THRESHOLD;
        if dark == current_dark {
            current_len += 1;
        } else {
            runs.push(current_len);
            current_dark = dark;
            current_len = 1;
        }
    }
    runs.push(current_len);

    let module_size = runs
        .into_iter()
        .filter(|length| *length >= 2)
        .reduce(greatest_common_divisor)
        .filter(|size| *size >= 2 && width.is_multiple_of(*size))
        .ok_or(())?;
    let modules = width / module_size;
    let quiet_modules = (min_x / module_size).min(min_y / module_size);
    let code_modules = modules
        .checked_sub(quiet_modules.checked_mul(2).ok_or(())?)
        .ok_or(())?;
    if !(21..=177).contains(&code_modules) || (code_modules - 21) % 4 != 0 {
        return Err(());
    }

    let matrix_len = modules.checked_mul(modules).ok_or(())?;
    let mut matrix = vec![false; matrix_len];
    let cell_area = module_size.checked_mul(module_size).ok_or(())?;
    for cell_y in 0..modules {
        for cell_x in 0..modules {
            let mut dark_pixels = 0usize;
            for y in cell_y * module_size..(cell_y + 1) * module_size {
                for x in cell_x * module_size..(cell_x + 1) * module_size {
                    dark_pixels += usize::from(pixels[y * width + x] < DARK_THRESHOLD);
                }
            }
            matrix[cell_y * modules + cell_x] = dark_pixels * 2 >= cell_area;
        }
    }

    let mut art = String::with_capacity(matrix_len.saturating_mul(3).saturating_add(modules / 2));
    for y in (0..modules).step_by(2) {
        for x in 0..modules {
            let top = matrix[y * modules + x];
            let bottom = y + 1 < modules && matrix[(y + 1) * modules + x];
            art.push(match (top, bottom) {
                (false, false) => ' ',
                (true, false) => '▀',
                (false, true) => '▄',
                (true, true) => '█',
            });
        }
        if y + 2 < modules {
            art.push('\n');
        }
    }
    Ok(art)
}

fn greatest_common_divisor(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

#[cfg(test)]
mod qr_image_tests {
    use super::*;

    #[test]
    fn renders_sampled_qr_grid() {
        let module_size = 2;
        let modules = 47;
        let width = module_size * modules;
        let mut pixels = vec![255; width * width];
        for module_y in 3..44 {
            for module_x in 3..44 {
                for y in module_y * module_size..(module_y + 1) * module_size {
                    for x in module_x * module_size..(module_x + 1) * module_size {
                        pixels[y * width + x] = 0;
                    }
                }
            }
        }

        let art = render_terminal_qr_luma(&pixels, width, width).unwrap();
        assert_eq!(art.lines().count(), 24);
        assert!(art.contains('█'));
    }

    #[test]
    fn rejects_non_qr_grid_dimensions() {
        let pixels = vec![0; 40 * 40];
        assert!(render_terminal_qr_luma(&pixels, 40, 40).is_err());
    }

    #[test]
    fn preserves_the_selected_index_for_single_images() {
        assert_eq!(
            download_strategy(ContentKind::ImageSet, 3, 0).unwrap(),
            DownloadStrategy::IndependentSet
        );
        assert_eq!(
            download_strategy(ContentKind::ImageSet, 1, 7).unwrap(),
            DownloadStrategy::IndexedSingle(7)
        );
        assert_eq!(
            download_strategy(ContentKind::Video, 3, 7).unwrap(),
            DownloadStrategy::PlayableFallback
        );
    }
}
