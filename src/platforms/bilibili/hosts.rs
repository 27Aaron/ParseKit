//! Reviewed media hosts and download request identity.

use crate::media::DownloadRequestIdentity;

pub(super) const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// Reviewed media hosts; a leading dot matches subdomains only.
pub const REVIEWED_MEDIA_HOSTS: &[&str] = &[
    ".bilivideo.com",
    ".bilivideo.cn",
    ".hdslb.com",
    "upos-sz-mirrorcos.bilivideo.com",
    "upos-sz-mirrorhw.bilivideo.com",
    "upos-sz-mirrorali.bilivideo.com",
    "upos-sz-estgcos.bilivideo.com",
    "upos-hz-mirrorakam.akamaized.net",
];

pub const REVIEWED_BILIBILI_MEDIA_HOSTS: &[&str] = REVIEWED_MEDIA_HOSTS;

pub fn download_identity() -> DownloadRequestIdentity {
    DownloadRequestIdentity {
        origin: Some("https://www.bilibili.com".into()),
        referer: Some("https://www.bilibili.com/".into()),
        user_agent: Some(USER_AGENT_VALUE.into()),
    }
}
