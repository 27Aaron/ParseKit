//! Parse and download media from supported social platforms.
//!
//! Use [`ParseKit`] as the facade or access individual [`platforms`] resolvers.

pub mod auth;
pub mod error;
pub mod hub;
pub mod media;
pub mod model;
pub mod platforms;
pub mod url;

pub mod wechat {
    pub use crate::platforms::wechat::*;
}

pub mod douyin {
    pub use crate::platforms::douyin::*;
}

pub mod bilibili {
    pub use crate::platforms::bilibili::*;
}

pub use auth::{
    CookieCredential, CredentialStatus, cookie_value, query_string_to_cookie_header,
    remove_dotenv_var, upsert_dotenv_var,
};
pub use error::{Error, Result};
pub use hub::{ParseKit, ParseKitBuilder};
pub use model::{
    ContentKind, MediaItem, MediaSource, MediaSourceKind, PlatformId, ResolvedPost, VideoCodec,
};
pub use platforms::bilibili::REVIEWED_BILIBILI_MEDIA_HOSTS;
pub use platforms::douyin::REVIEWED_DOUYIN_MEDIA_HOSTS;
pub use platforms::wechat::REVIEWED_WECHAT_MEDIA_HOSTS;
pub use platforms::{
    BilibiliResolver, DouyinResolver, PLATFORM_SPECS, Platform, PlatformResolver, PlatformSpec,
    WechatResolver, platform_spec,
};
