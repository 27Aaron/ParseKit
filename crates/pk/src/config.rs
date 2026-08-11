//! Env loading and kit construction.

use std::path::PathBuf;

use parse_kit::{ParseKit, ParseKitBuilder, Result};

const DEFAULT_MAX_BYTES: u64 = 200 * 1024 * 1024;

pub fn load_dotenv() {
    let _ = dotenvy::from_filename(".env.local");
    let _ = dotenvy::dotenv();
}

pub fn build_kit() -> Result<ParseKit> {
    let mut builder = ParseKitBuilder::new();
    match std::env::var("YUANBAO_COOKIE") {
        Ok(cookie) if !cookie.trim().is_empty() => {
            builder = builder.wechat(cookie)?;
        }
        _ => {}
    }
    builder = builder.douyin()?;
    builder = builder.bilibili()?;
    builder.build()
}

pub fn default_output_dir() -> PathBuf {
    std::env::var_os("PARSE_KIT_OUTPUT_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("./downloads"))
}

pub fn default_max_bytes() -> u64 {
    std::env::var("PARSE_KIT_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_MAX_BYTES)
}
