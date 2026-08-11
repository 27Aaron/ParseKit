//! Multi-platform social media resolve and download core.
//!
//! This crate is delivery-agnostic: it does not know about Telegram, Feishu, or
//! other chat products. Apps depend on [`ParseHub`] plus [`media`] helpers.
//!
//! # Platforms
//!
//! Concrete resolvers live under [`platforms`]. Register new ones via the
//! [`platforms::Platform`] enum and [`ParseHub::builder`] — see the module docs.
//!
//! Platform-private pieces (CDN host allowlists, WeChat decrypt, cookies) stay
//! under each platform module rather than shared `model` / `media`.

pub mod error;
pub mod hub;
pub mod media;
pub mod model;
pub mod platforms;

/// Backward-compatible path for WeChat Channels (`parse_core::wechat::…`).
pub mod wechat {
    pub use crate::platforms::wechat::*;
}

/// Convenience path for Douyin (`parse_core::douyin::…`).
pub mod douyin {
    pub use crate::platforms::douyin::*;
}

pub use error::{Error, Result};
pub use hub::ParseHub;
pub use model::{MediaSource, MediaSourceKind, ResolvedPost, VideoCodec};
pub use platforms::{DouyinResolver, Platform, PlatformResolver, WechatResolver};

// Platform CDN allowlists re-exported for older call sites.
pub use platforms::douyin::REVIEWED_DOUYIN_MEDIA_HOSTS;
pub use platforms::wechat::REVIEWED_WECHAT_MEDIA_HOSTS;
