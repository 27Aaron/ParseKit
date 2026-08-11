//! Private file creation, resumable writes, and streaming prefix decryption.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
};

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{Error, Result};

use super::progress::ProgressReporter;
use super::{DiskWriteBudget, DownloadedMedia};

pub(super) const MIN_FREE_DISK_BYTES: u64 = 512 * 1024 * 1024;
const DISK_CHECK_INTERVAL_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn media_task_path(
    directory: &Path,
    url: &url::Url,
    file_stem: Option<&str>,
    sequence: u32,
) -> PathBuf {
    let ext = extension_from_url(url);
    let name = match file_stem.and_then(safe_file_stem) {
        Some(stem) => {
            if sequence == 0 {
                format!("{stem}.{ext}")
            } else {
                format!("{stem}_{sequence}.{ext}")
            }
        }
        _ => format!("{}.{ext}", Uuid::new_v4().hyphenated()),
    };
    directory.join(name)
}

pub(super) fn safe_file_stem(raw: &str) -> Option<String> {
    let stem = crate::model::sanitize_filename_component(raw);
    (!stem.is_empty()).then_some(stem)
}

const EXISTING_MEDIA_EXTS: &[&str] = &[
    "mp4", "m4v", "mov", "m4s", "webm", "mkv", "flv", "mpeg", "mpg", "ts", "jpg", "jpeg", "png",
    "webp", "gif", "avif", "heic", "mp3", "m4a", "aac", "ogg", "bin",
];

/// Looks for a complete on-disk file for `{stem}` / `{stem}_{n}` under `directory`.
pub(super) async fn existing_complete_download(
    directory: &Path,
    file_stem: Option<&str>,
    sequence: u32,
    preferred_ext: &str,
    size_hint: Option<u64>,
) -> Option<(PathBuf, u64)> {
    let stem = file_stem.and_then(safe_file_stem)?;
    let base = if sequence == 0 {
        stem
    } else {
        format!("{stem}_{sequence}")
    };

    let mut exts = Vec::with_capacity(EXISTING_MEDIA_EXTS.len() + 1);
    exts.push(preferred_ext);
    for ext in EXISTING_MEDIA_EXTS {
        if *ext != preferred_ext {
            exts.push(*ext);
        }
    }

    for ext in exts {
        let path = directory.join(format!("{base}.{ext}"));
        // Never reuse a symlink: callers should only receive regular files that
        // are actually contained in the download workspace.
        let Ok(meta) = tokio::fs::symlink_metadata(&path).await else {
            continue;
        };
        if !meta.is_file() || meta.file_type().is_symlink() {
            continue;
        }
        let len = meta.len();
        if len == 0 {
            continue;
        }
        if let Some(hint) = size_hint {
            if len >= hint && has_recognizable_media_header(&path).await {
                return Some((path, len));
            }
            // Incomplete relative to size_hint — allow resume/re-download.
            continue;
        }
        if looks_like_complete_media(&path, len).await {
            return Some((path, len));
        }
    }
    None
}

async fn looks_like_complete_media(path: &Path, len: u64) -> bool {
    if len < 1024 {
        return false;
    }
    has_recognizable_media_header(path).await
}

async fn has_recognizable_media_header(path: &Path) -> bool {
    if crate::media::file_prefix_looks_like_bmff(path)
        .await
        .unwrap_or(false)
    {
        return true;
    }
    match tokio::fs::File::open(path).await {
        Ok(mut file) => {
            use tokio::io::AsyncReadExt;
            let mut header = [0_u8; 512];
            let Ok(n) = file.read(&mut header).await else {
                return false;
            };
            looks_like_media_header(&header[..n])
        }
        Err(_) => false,
    }
}

pub(super) fn looks_like_media_header(header: &[u8]) -> bool {
    header.starts_with(&[0xff, 0xd8, 0xff]) // JPEG
        || header.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']) // PNG
        || header.starts_with(b"GIF87a")
        || header.starts_with(b"GIF89a")
        || (header.len() >= 12 && header.starts_with(b"RIFF") && &header[8..12] == b"WEBP")
        || header.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) // Matroska / WebM
        || header.starts_with(b"FLV")
        || header.starts_with(b"OggS")
        || header.starts_with(&[0, 0, 1, 0xba]) // MPEG program stream
        || header.starts_with(&[0, 0, 1, 0xb3]) // MPEG video sequence
        || header.starts_with(b"ID3")
        || looks_like_mpeg_audio_frame(header)
        || looks_like_aac_adts_frame(header)
        || (header.len() > 376
            && header[0] == 0x47
            && header[188] == 0x47
            && header[376] == 0x47) // MPEG transport stream
}

fn looks_like_aac_adts_frame(header: &[u8]) -> bool {
    header.len() >= 7 && header[0] == 0xff && header[1] & 0xf6 == 0xf0 && header[2] & 0x3c != 0x3c
}

fn looks_like_mpeg_audio_frame(header: &[u8]) -> bool {
    if header.len() < 4 || header[0] != 0xff || header[1] & 0xe0 != 0xe0 {
        return false;
    }
    let version = header[1] & 0x18;
    let layer = header[1] & 0x06;
    let bitrate = header[2] & 0xf0;
    let sample_rate = header[2] & 0x0c;
    version != 0x08 && layer != 0 && !matches!(bitrate, 0 | 0xf0) && sample_rate != 0x0c
}

/// Infers a safe extension from the media URL, falling back to `bin`.
///
/// WeChat Channels CDN paths are often extension-less (`…/stodownload?…`);
/// Douyin uses `…/play/?video_id=…`. Prefer a real path suffix when present.
pub(super) fn extension_from_url(url: &url::Url) -> &'static str {
    let path = url.path().to_ascii_lowercase();
    if let Some(ext) = extension_from_path_segment(&path) {
        return ext;
    }

    let query = url.query().unwrap_or("").to_ascii_lowercase();
    // finder.video.qq.com/…/stodownload — video vs cover distinguished by query.
    if path.contains("stodownload") {
        if query.contains("picformat") || query.contains("wxampic") {
            return "jpg";
        }
        return "mp4";
    }
    if path.contains("play")
        || path.contains("video")
        || path.contains("stream")
        || path.contains("media")
        || path.contains("download")
    {
        return "mp4";
    }
    if path.contains("image")
        || path.contains("cover")
        || path.contains("thumb")
        || path.contains("pic")
    {
        return "jpg";
    }
    "bin"
}

fn extension_from_path_segment(path: &str) -> Option<&'static str> {
    let segment = path.rsplit('/').next().unwrap_or("");
    let (name, ext) = segment.rsplit_once('.')?;
    if name.is_empty() || ext.is_empty() || ext.len() > 5 {
        return None;
    }
    Some(match ext {
        "mp4" | "m4v" | "mov" => "mp4",
        "m4s" | "mpd" => "m4s",
        "flv" => "flv",
        "webm" => "webm",
        "mkv" => "mkv",
        "mpeg" | "mpg" => "mpg",
        "jpg" | "jpeg" => "jpg",
        "png" => "png",
        "webp" => "webp",
        "gif" => "gif",
        "avif" => "avif",
        "heic" | "heif" => "heic",
        "ts" => "ts",
        "mp3" => "mp3",
        "m4a" => "m4a",
        "aac" => "aac",
        "ogg" | "oga" | "ogv" => "ogg",
        _ => return None,
    })
}

/// Maps a response `Content-Type` to a file extension when the URL had none.
pub(super) fn extension_from_content_type(value: &str) -> Option<&'static str> {
    let mime = value
        .split(';')
        .next()
        .unwrap_or(value)
        .trim()
        .to_ascii_lowercase();
    Some(match mime.as_str() {
        "video/mp4" | "application/mp4" => "mp4",
        "video/mpeg" => "mpg",
        "video/mp2t" => "ts",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "video/x-flv" | "video/flv" => "flv",
        "video/quicktime" => "mp4",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/avif" => "avif",
        "image/heic" | "image/heif" => "heic",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" => "m4a",
        "audio/aac" => "aac",
        "audio/ogg" | "video/ogg" | "application/ogg" => "ogg",
        _ => return None,
    })
}

/// Prefer a real extension over a provisional `bin` name.
pub(super) fn path_with_better_extension(path: PathBuf, preferred: &str) -> PathBuf {
    if preferred.is_empty() || preferred == "bin" {
        return path;
    }
    let current = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("bin");
    if current == "bin" {
        path.with_extension(preferred)
    } else {
        path
    }
}

/// Returns the safe resume offset for a response, or `None` if it cannot resume.
///
/// An offset of zero means the destination must be rewritten from the start.
pub(super) fn effective_resume_offset(requested: u64, status: reqwest::StatusCode) -> Option<u64> {
    use reqwest::StatusCode;
    if requested == 0 {
        return Some(0);
    }
    if status == StatusCode::PARTIAL_CONTENT {
        return Some(requested);
    }
    if status == StatusCode::OK {
        // A 200 response ignored `Range`; restart from byte zero.
        return Some(0);
    }
    None
}

#[cfg(unix)]
pub(super) fn ensure_free_disk_space(directory: &Path, pending_write_bytes: u64) -> Result<()> {
    use std::{ffi::CString, mem::MaybeUninit, os::unix::ffi::OsStrExt};

    let path = CString::new(directory.as_os_str().as_bytes())
        .map_err(|_| Error::Storage(directory.to_owned()))?;
    let mut statistics = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is a valid NUL-terminated C string, and `statistics`
    // points to aligned, writable memory for `statvfs` to initialize.
    if unsafe { libc::statvfs(path.as_ptr(), statistics.as_mut_ptr()) } != 0 {
        return Err(Error::Storage(directory.to_owned()));
    }
    // SAFETY: `statvfs` succeeded, so `statistics` is initialized.
    let statistics = unsafe { statistics.assume_init() };
    let block_size = if statistics.f_frsize == 0 {
        statistics.f_bsize
    } else {
        statistics.f_frsize
    };
    let available_bytes = u128::from(statistics.f_bavail) * u128::from(block_size);
    if !disk_space_is_sufficient(available_bytes, pending_write_bytes) {
        return Err(Error::Storage(directory.to_owned()));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn disk_space_is_sufficient(available_bytes: u128, pending_write_bytes: u64) -> bool {
    let required_bytes = u128::from(MIN_FREE_DISK_BYTES.saturating_add(pending_write_bytes));
    available_bytes >= required_bytes
}

#[cfg(not(unix))]
pub(super) fn ensure_free_disk_space(_directory: &Path, _pending_write_bytes: u64) -> Result<()> {
    Ok(())
}

pub(super) async fn create_private_file(path: PathBuf) -> Result<PendingFile> {
    let storage_path = path.clone();
    tokio::task::spawn_blocking(move || {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        options.open(&path).map(|file| PendingFile::new(path, file))
    })
    .await
    .map_err(|_| Error::Storage(storage_path.clone()))?
    .map_err(|_| Error::Storage(storage_path))
}

pub(super) async fn open_private_file_append(
    path: PathBuf,
    expected_length: u64,
) -> Result<PendingFile> {
    let storage_path = path.clone();
    tokio::task::spawn_blocking(move || {
        let mut options = OpenOptions::new();
        options.write(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() != expected_length {
            return Err(std::io::Error::other(
                "media destination is not a regular file",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(std::io::Error::other(
                    "media destination has multiple hard links",
                ));
            }
        }
        Ok(PendingFile::new(path, file))
    })
    .await
    .map_err(|_| Error::Storage(storage_path.clone()))?
    .map_err(|_| Error::Storage(storage_path))
}

pub(super) fn write_chunks<T: AsRef<[u8]>>(
    mut pending_file: PendingFile,
    receiver: &mut mpsc::Receiver<T>,
    mut progress_reporter: Option<ProgressReporter>,
    disk_write_budget: Arc<StdMutex<DiskWriteBudget>>,
    mut prefix_xor: Option<crate::platforms::wechat::PrefixXor>,
    initial_bytes: u64,
) -> Result<WrittenMedia> {
    let mut bytes = initial_bytes;
    let mut checked_disk_space = false;

    while let Some(chunk) = receiver.blocking_recv() {
        if prefix_xor.as_ref().is_some_and(|xor| xor.remaining == 0) {
            prefix_xor = None;
        }
        let chunk = chunk.as_ref();
        let mut decrypted = Vec::new();
        let buffer = if let Some(xor) = prefix_xor.as_mut() {
            decrypted.extend_from_slice(chunk);
            xor.transform(&mut decrypted);
            decrypted.as_slice()
        } else {
            chunk
        };
        let next_bytes = bytes
            .checked_add(buffer.len() as u64)
            .ok_or_else(|| Error::Download("媒体大小计算溢出".into()))?;

        let chunk_bytes = buffer.len() as u64;
        let mut disk_budget = disk_write_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let projected_bytes = disk_budget.unchecked_bytes.saturating_add(chunk_bytes);
        let check_disk_space = !checked_disk_space
            || disk_budget.unchecked_bytes == 0
            || projected_bytes > DISK_CHECK_INTERVAL_BYTES;
        if check_disk_space {
            ensure_free_disk_space(
                pending_file
                    .path
                    .parent()
                    .ok_or_else(|| Error::Storage(pending_file.path.clone()))?,
                DISK_CHECK_INTERVAL_BYTES.max(chunk_bytes),
            )?;
            disk_budget.unchecked_bytes = 0;
            checked_disk_space = true;
        }
        disk_budget.unchecked_bytes = if chunk_bytes >= DISK_CHECK_INTERVAL_BYTES {
            0
        } else {
            disk_budget.unchecked_bytes.saturating_add(chunk_bytes)
        };
        drop(disk_budget);

        pending_file.file_mut()?.write_all(buffer)?;
        bytes = next_bytes;
        if let Some(reporter) = &mut progress_reporter {
            reporter.report_intermediate(bytes);
        }
    }

    let media = pending_file.finish(bytes)?;
    Ok(WrittenMedia {
        media,
        progress_reporter,
    })
}

pub(super) struct WrittenMedia {
    pub(super) media: DownloadedMedia,
    pub(super) progress_reporter: Option<ProgressReporter>,
}

pub(super) struct PendingFile {
    path: PathBuf,
    file: Option<File>,
    armed: bool,
}

impl PendingFile {
    pub(super) fn new(path: PathBuf, file: File) -> Self {
        Self {
            path,
            file: Some(file),
            armed: true,
        }
    }

    fn file_mut(&mut self) -> Result<&mut File> {
        self.file
            .as_mut()
            .ok_or_else(|| Error::Download("临时媒体文件已经关闭".to_owned()))
    }

    fn finish(mut self, bytes: u64) -> Result<DownloadedMedia> {
        let file = self.file_mut()?;
        file.flush()?;
        drop(self.file.take());
        self.armed = false;
        Ok(DownloadedMedia::new(self.path.clone(), bytes))
    }
}

impl Drop for PendingFile {
    fn drop(&mut self) {
        drop(self.file.take());
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
