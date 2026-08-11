//! Disk write path, private temp files, and stream XOR while writing.

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

pub(super) fn random_task_path(directory: &Path, url: &url::Url) -> PathBuf {
    let ext = extension_from_url(url);
    directory.join(format!("{}.{ext}", Uuid::new_v4().hyphenated()))
}

/// Guess a safe file extension from the media URL path (default `bin`).
pub(super) fn extension_from_url(url: &url::Url) -> &'static str {
    let path = url.path().to_ascii_lowercase();
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "mp4" | "m4v" | "mov" => "mp4",
        "m4s" | "mpd" => "m4s",
        "flv" => "flv",
        "webm" => "webm",
        "jpg" | "jpeg" => "jpg",
        "png" => "png",
        "webp" => "webp",
        "gif" => "gif",
        "ts" => "ts",
        _ => {
            if path.contains("play") || path.contains("video") {
                "mp4"
            } else if path.contains("image") || path.contains("cover") {
                "jpg"
            } else {
                "bin"
            }
        }
    }
}

/// Decide resume offset handling after an HTTP response status is known.
/// Returns the effective resume offset (0 = rewrite from start).
pub(super) fn effective_resume_offset(requested: u64, status: reqwest::StatusCode) -> Option<u64> {
    use reqwest::StatusCode;
    if requested == 0 {
        return Some(0);
    }
    if status == StatusCode::PARTIAL_CONTENT {
        return Some(requested);
    }
    if status == StatusCode::OK {
        // Server ignored Range — caller should restart at 0.
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
    // SAFETY: `path` is NUL-terminated and `statistics` points to writable,
    // correctly aligned storage. A successful call initializes the structure.
    if unsafe { libc::statvfs(path.as_ptr(), statistics.as_mut_ptr()) } != 0 {
        return Err(Error::Storage(directory.to_owned()));
    }
    // SAFETY: statvfs returned success immediately above.
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
            options.mode(0o600);
        }
        options.open(&path).map(|file| PendingFile::new(path, file))
    })
    .await
    .map_err(|_| Error::Storage(storage_path.clone()))?
    .map_err(|_| Error::Storage(storage_path))
}

pub(super) async fn open_private_file_append(path: PathBuf) -> Result<PendingFile> {
    let storage_path = path.clone();
    tokio::task::spawn_blocking(move || {
        let mut options = OpenOptions::new();
        options.write(true).append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&path).map(|file| PendingFile::new(path, file))
    })
    .await
    .map_err(|_| Error::Storage(storage_path.clone()))?
    .map_err(|_| Error::Storage(storage_path))
}

pub(super) fn write_chunks<T: AsRef<[u8]>>(
    mut pending_file: PendingFile,
    receiver: &mut mpsc::Receiver<T>,
    max_bytes: u64,
    mut progress_reporter: Option<ProgressReporter>,
    disk_write_budget: Arc<StdMutex<DiskWriteBudget>>,
    mut prefix_xor: Option<crate::platforms::wechat::PrefixXor>,
    initial_bytes: u64,
) -> Result<WrittenMedia> {
    let mut bytes = initial_bytes;
    let mut checked_disk_space = false;

    while let Some(chunk) = receiver.blocking_recv() {
        let mut buffer = chunk.as_ref().to_vec();
        if let Some(xor) = prefix_xor.as_mut() {
            xor.transform(&mut buffer);
        }
        let next_bytes = bytes
            .checked_add(buffer.len() as u64)
            .ok_or(Error::MediaTooLarge {
                actual: u64::MAX,
                limit: max_bytes,
            })?;
        if next_bytes > max_bytes {
            return Err(Error::MediaTooLarge {
                actual: next_bytes,
                limit: max_bytes,
            });
        }

        let chunk_bytes = buffer.len() as u64;
        let mut disk_budget = disk_write_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let projected_bytes = disk_budget.unchecked_bytes.saturating_add(chunk_bytes);
        if !checked_disk_space
            || disk_budget.unchecked_bytes == 0
            || projected_bytes > DISK_CHECK_INTERVAL_BYTES
        {
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
        pending_file.file_mut()?.write_all(&buffer)?;
        disk_budget.unchecked_bytes = if chunk_bytes >= DISK_CHECK_INTERVAL_BYTES {
            0
        } else {
            disk_budget.unchecked_bytes.saturating_add(chunk_bytes)
        };
        drop(disk_budget);
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
