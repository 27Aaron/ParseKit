//! Reviewed media hosts and download request identity.

use crate::media::DownloadRequestIdentity;

const MEDIA_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const DOUYIN_ORIGIN: &str = "https://www.douyin.com";
const DOUYIN_REFERER: &str = "https://www.douyin.com/";

/// Reviewed media hosts; dot-prefixed entries match subdomains.
pub const REVIEWED_MEDIA_HOSTS: &[&str] = &[
    "aweme.snssdk.com",
    "www.douyin.com",
    "www.iesdouyin.com",
    ".douyinvod.com",
    ".douyincdn.com",
    // Play URLs may redirect through jspcdn on port 20443.
    ".jspcdn.cn",
    ".bytevcloudcdn.com",
    ".bytecdn.cn",
    ".bytecdn.com",
    ".zjcdn.com",
    ".douyinpic.com",
    ".ibyteimg.com",
    ".pstatp.com",
];

pub const REVIEWED_DOUYIN_MEDIA_HOSTS: &[&str] = REVIEWED_MEDIA_HOSTS;

pub fn download_identity() -> DownloadRequestIdentity {
    DownloadRequestIdentity {
        origin: Some(DOUYIN_ORIGIN.to_owned()),
        referer: Some(DOUYIN_REFERER.to_owned()),
        user_agent: Some(MEDIA_USER_AGENT.to_owned()),
    }
}
