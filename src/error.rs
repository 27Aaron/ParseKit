use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("配置错误：{0}")]
    Config(String),

    #[error("暂不支持这个链接")]
    UnsupportedUrl,

    #[error("解析凭据已失效，请更新登录信息")]
    LoginRequired,

    #[error("内容不存在、已删除或不可见")]
    NotFound,

    #[error("该视频暂时无法取得可用媒体地址")]
    MediaUnavailable,

    #[error("上游接口结构可能已经变化")]
    UpstreamChanged,

    #[error("上游请求过于频繁，请稍后再试")]
    RateLimited,

    #[error("网络请求失败：{0}")]
    Network(String),

    #[error("下载失败：{0}")]
    Download(String),

    #[error("媒体文件无效：{0}")]
    InvalidMedia(String),

    #[error("临时目录不可用：{0}")]
    Storage(PathBuf),

    #[error("任务已过期，请重新发送链接")]
    Expired,

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
