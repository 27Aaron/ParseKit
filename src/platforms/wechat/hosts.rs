//! CDN hosts and download request identity.

use crate::media::DownloadRequestIdentity;

/// Reviewed CDN hosts for media download (exact match only).
pub const REVIEWED_MEDIA_HOSTS: &[&str] = &[
    "finder.video.qq.com",
    "findermp.video.qq.com",
    "finder.video.wechat.com",
    "findermp.video.wechat.com",
];

pub const REVIEWED_WECHAT_MEDIA_HOSTS: &[&str] = REVIEWED_MEDIA_HOSTS;

const CHANNELS_ORIGIN: &str = "https://channels.weixin.qq.com";
const CHANNELS_REFERER: &str = "https://channels.weixin.qq.com/";
const MEDIA_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

pub fn download_identity() -> DownloadRequestIdentity {
    DownloadRequestIdentity {
        origin: Some(CHANNELS_ORIGIN.to_owned()),
        referer: Some(CHANNELS_REFERER.to_owned()),
        user_agent: Some(MEDIA_USER_AGENT.to_owned()),
    }
}
