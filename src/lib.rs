//! Multi-platform parse and media download library.
//!
//! Resolvers: [`platforms`]. Facade: [`ParseKit`].

pub mod error;
pub mod hub;
pub mod media;
pub mod model;
pub mod platforms;

pub mod wechat {
    pub use crate::platforms::wechat::*;
}

pub mod douyin {
    pub use crate::platforms::douyin::*;
}

pub use error::{Error, Result};
pub use hub::{ParseKit, ParseKitBuilder};
pub use model::{ContentKind, MediaItem, MediaSource, MediaSourceKind, ResolvedPost, VideoCodec};
pub use platforms::douyin::REVIEWED_DOUYIN_MEDIA_HOSTS;
pub use platforms::wechat::REVIEWED_WECHAT_MEDIA_HOSTS;
pub use platforms::{DouyinResolver, Platform, PlatformResolver, WechatResolver};
