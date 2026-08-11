//! WeChat Channels platform adapter and media helpers.

mod api;
mod decrypt;
mod hosts;
mod parse;
mod resolver;
mod share;

#[cfg(test)]
mod tests;

use super::PlatformSpec;
use crate::PlatformId;

pub(crate) use decrypt::PrefixXor;
pub use decrypt::decrypt_file_prefix;
pub use hosts::{REVIEWED_MEDIA_HOSTS, REVIEWED_WECHAT_MEDIA_HOSTS, download_identity};
pub use resolver::{WechatCredentialStatus, WechatResolver, assess_yuanbao_cookie};
pub use share::{derive_direct_media_url, extract_share_url};

/// Complete static registration for the WeChat Channels adapter.
pub const SPEC: PlatformSpec = PlatformSpec::new(
    PlatformId::Wechat,
    "needs YUANBAO_COOKIE (pk wechat login)",
    extract_share_url,
    REVIEWED_MEDIA_HOSTS,
    download_identity,
);
