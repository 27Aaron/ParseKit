//! Shared media download and probe utilities.
//!
//! CDN host allowlists and request identities live under each platform module
//! (`platforms::wechat`, `platforms::douyin`). Convenience constructors on
//! [`MediaDownloader`] pull from those modules.
//!
//! WeChat Channels prefix decryption is platform-private; it is re-exported
//! here only for a stable `media::decrypt_file_prefix` import path.

pub mod downloader;
pub mod probe;

pub use downloader::{DownloadProgress, DownloadRequestIdentity, DownloadedMedia, MediaDownloader};
pub use probe::{MediaProbe, probe_media};

/// WeChat Channels media prefix decrypt (platform-private implementation).
///
/// Prefer `parse_kit::wechat::decrypt_file_prefix` when the call site already
/// knows it is handling WeChat media.
pub use crate::platforms::wechat::decrypt_file_prefix;
