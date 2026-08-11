//! WeChat Channels (视频号).

mod decrypt;
mod hosts;
mod resolve;

pub use decrypt::decrypt_file_prefix;
pub use hosts::{REVIEWED_MEDIA_HOSTS, REVIEWED_WECHAT_MEDIA_HOSTS, download_identity};
pub use resolve::{WechatResolver, derive_direct_media_url, extract_share_url};
