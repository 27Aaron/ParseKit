//! WeChat Channels (微信视频号) resolver and media helpers.
//!
//! Platform-private pieces live here so the shared `model` / `media` layers stay
//! free of Yuanbao endpoints, CDN hosts, and decode-key crypto.

mod decrypt;
mod hosts;
mod resolve;

pub use decrypt::decrypt_file_prefix;
pub use hosts::{REVIEWED_MEDIA_HOSTS, REVIEWED_WECHAT_MEDIA_HOSTS, download_identity};
pub use resolve::{WechatResolver, derive_direct_media_url, extract_share_url};
