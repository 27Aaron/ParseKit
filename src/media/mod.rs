//! Download, probe, and WeChat decrypt re-export.

pub mod downloader;
pub mod probe;

pub use downloader::{DownloadProgress, DownloadRequestIdentity, DownloadedMedia, MediaDownloader};
pub use probe::{MediaProbe, probe_media};

/// WeChat prefix decrypt (implementation lives under `platforms::wechat`).
pub use crate::platforms::wechat::decrypt_file_prefix;
