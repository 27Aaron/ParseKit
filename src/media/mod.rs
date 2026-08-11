//! Media download, validation, probing, and decryption utilities.

pub mod bmff;
pub mod downloader;
pub(crate) mod host;
pub mod probe;

pub use bmff::{file_prefix_looks_like_bmff, looks_like_bmff, prefix_looks_like_bmff};
pub use downloader::{
    DownloadProgress, DownloadRequestIdentity, DownloadedMedia, MediaDownloader,
    enrich_missing_size_hints,
};
pub use probe::{MediaProbe, probe_media};

/// Decrypts the encrypted prefix used by WeChat Channels media.
pub use crate::platforms::wechat::decrypt_file_prefix;
