//! Load environment configuration and construct `ParseKit`.

use std::path::{Path, PathBuf};

use parse_kit::{ParseKit, ParseKitBuilder, Result};

/// Env key for Bilibili web cookie (`SESSDATA=...; bili_jct=...`).
pub const BILIBILI_COOKIE_ENV: &str = "BILIBILI_COOKIE";
/// Env key for WeChat Channels via Yuanbao (`hy_user` / `hy_token` / …).
pub const YUANBAO_COOKIE_ENV: &str = "YUANBAO_COOKIE";

/// Preferred path for CLI-written credentials (does not clobber hand-edited `.env`).
pub fn env_local_path() -> PathBuf {
    PathBuf::from(".env.local")
}

pub fn load_dotenv() {
    let _ = dotenvy::from_filename(".env.local");
    let _ = dotenvy::dotenv();
}

pub fn build_kit() -> Result<ParseKit> {
    let mut builder = ParseKitBuilder::new();
    match std::env::var(YUANBAO_COOKIE_ENV) {
        Ok(cookie) if !cookie.trim().is_empty() => {
            builder = builder.wechat(cookie)?;
        }
        _ => {}
    }
    builder = builder.douyin()?;
    match std::env::var(BILIBILI_COOKIE_ENV) {
        Ok(cookie) if !cookie.trim().is_empty() => {
            builder = builder.bilibili_with_cookie(cookie)?;
        }
        _ => {
            builder = builder.bilibili()?;
        }
    }
    builder.build()
}

pub fn default_output_dir() -> PathBuf {
    std::env::var_os("PARSE_KIT_OUTPUT_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("./downloads"))
}

/// Persist a cookie env key into `.env.local` and the current process env.
pub fn save_cookie_env(key: &str, cookie: &str) -> Result<()> {
    let path = env_local_path();
    parse_kit::auth::upsert_dotenv_var(&path, key, cookie).map_err(|error| {
        parse_kit::Error::Config(format!("无法写入 {}: {error}", path.display()))
    })?;
    // SAFETY: single-threaded CLI entry after dotenv load; only mutates our keys.
    unsafe {
        std::env::set_var(key, cookie);
    }
    Ok(())
}

/// Remove a cookie env key from `.env.local` (and process env).
pub fn clear_cookie_env(key: &str) -> Result<bool> {
    let path = env_local_path();
    let removed = if path.exists() {
        parse_kit::auth::remove_dotenv_var(Path::new(&path), key).map_err(|error| {
            parse_kit::Error::Config(format!("无法更新 {}: {error}", path.display()))
        })?
    } else {
        false
    };
    unsafe {
        std::env::remove_var(key);
    }
    Ok(removed)
}

pub fn save_bilibili_cookie(cookie: &str) -> Result<()> {
    save_cookie_env(BILIBILI_COOKIE_ENV, cookie)
}

pub fn clear_bilibili_cookie() -> Result<bool> {
    clear_cookie_env(BILIBILI_COOKIE_ENV)
}

pub fn save_yuanbao_cookie(cookie: &str) -> Result<()> {
    save_cookie_env(YUANBAO_COOKIE_ENV, cookie)
}

pub fn clear_yuanbao_cookie() -> Result<bool> {
    clear_cookie_env(YUANBAO_COOKIE_ENV)
}
