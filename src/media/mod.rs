//! Download, probe, BMFF checks, and WeChat decrypt re-export.

pub mod bmff;
pub mod downloader;
pub mod probe;

pub use bmff::{file_prefix_looks_like_bmff, looks_like_bmff, prefix_looks_like_bmff};
pub use downloader::{DownloadProgress, DownloadRequestIdentity, DownloadedMedia, MediaDownloader};
pub use probe::{MediaProbe, probe_media};

/// WeChat prefix decrypt (implementation lives under `platforms::wechat`).
pub use crate::platforms::wechat::decrypt_file_prefix;
