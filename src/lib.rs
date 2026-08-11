//! Multi-platform social media resolve and download core.
//!
//! This crate is delivery-agnostic: it does not know about Telegram, Feishu, or
//! other chat products. Apps depend on [`ParseHub`] plus [`media`] helpers.
//!
//! # Platforms
//!
//! Concrete resolvers live under [`platforms`]. Register new ones via the
//! [`platforms::Platform`] enum and [`ParseHub::new`] — see the module docs.

pub mod error;
pub mod hub;
pub mod media;
pub mod model;
pub mod platforms;

/// Backward-compatible path for WeChat Channels (`parse_core::wechat::…`).
pub mod wechat {
    pub use crate::platforms::wechat::*;
}

pub use error::{Error, Result};
pub use hub::ParseHub;
pub use model::{
    MediaSource, MediaSourceKind, REVIEWED_WECHAT_MEDIA_HOSTS, ResolvedPost, VideoCodec,
};
pub use platforms::{Platform, PlatformResolver};
