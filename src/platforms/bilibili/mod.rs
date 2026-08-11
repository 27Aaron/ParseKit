//! Bilibili platform adapter.

mod hosts;
mod parse;
mod resolver;
mod share;

#[cfg(test)]
mod tests;

use super::PlatformSpec;
use crate::PlatformId;

pub use hosts::{REVIEWED_BILIBILI_MEDIA_HOSTS, REVIEWED_MEDIA_HOSTS, download_identity};
pub use resolver::BilibiliResolver;
pub use share::extract_share_url;

/// Complete static registration for the Bilibili adapter.
pub const SPEC: PlatformSpec = PlatformSpec::new(
    PlatformId::Bilibili,
    "public video page",
    extract_share_url,
    REVIEWED_MEDIA_HOSTS,
    download_identity,
);
