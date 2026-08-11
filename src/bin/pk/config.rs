//! Load environment configuration and construct `ParseKit`.

use std::{
    env,
    path::{Path, PathBuf},
};

use parse_kit::{Error, ParseKit, ParseKitBuilder, Result};

const BILIBILI_COOKIE_ENV: &str = "BILIBILI_COOKIE";
const YUANBAO_COOKIE_ENV: &str = "YUANBAO_COOKIE";
const ENV_LOCAL_FILE: &str = ".env.local";
const ENV_FILE: &str = ".env";

/// Returns the CLI-managed credential file.
pub fn env_local_path() -> &'static Path {
    Path::new(ENV_LOCAL_FILE)
}

pub fn load_dotenv() -> Result<()> {
    load_dotenv_file(env_local_path())?;
    load_dotenv_file(Path::new(ENV_FILE))
}

fn load_dotenv_file(path: &Path) -> Result<()> {
    match dotenvy::from_path(path) {
        Ok(_) => Ok(()),
        Err(error) if error.not_found() => Ok(()),
        Err(error) => {
            let detail = dotenv_error_detail(error);
            Err(Error::Config(format!(
                "无法加载 {}: {detail}",
                path.display()
            )))
        }
    }
}

fn dotenv_error_detail(error: dotenvy::Error) -> String {
    match error {
        // Avoid exposing the source line, which may contain credentials.
        dotenvy::Error::LineParse(_, offset) => format!("格式错误（位置 {offset}）"),
        dotenvy::Error::Io(error) => error.to_string(),
        dotenvy::Error::EnvVar(error) => error.to_string(),
        _ => "未知错误".into(),
    }
}

pub fn build_kit() -> Result<ParseKit> {
    let mut builder = ParseKitBuilder::new();
    if let Some(cookie) = cookie_from_env(YUANBAO_COOKIE_ENV)? {
        builder = builder.wechat(cookie)?;
    }
    builder = builder.douyin()?;
    if let Some(cookie) = cookie_from_env(BILIBILI_COOKIE_ENV)? {
        builder = builder.bilibili_with_cookie(cookie)?;
    } else {
        builder = builder.bilibili()?;
    }
    builder.build()
}

fn cookie_from_env(key: &str) -> Result<Option<String>> {
    match env::var(key) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(Error::Config(format!("{key} 必须是有效的 Unicode 字符串")))
        }
    }
}

pub fn default_output_dir() -> PathBuf {
    env::var_os("PARSE_KIT_OUTPUT_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("./downloads"))
}

fn save_cookie_env(key: &str, cookie: &str) -> Result<()> {
    let path = env_local_path();
    parse_kit::auth::upsert_dotenv_var(path, key, cookie)
        .map_err(|error| Error::Config(format!("无法写入 {}: {error}", path.display())))?;
    Ok(())
}

fn clear_cookie_env(key: &str) -> Result<bool> {
    let path = env_local_path();
    parse_kit::auth::remove_dotenv_var(path, key)
        .map_err(|error| Error::Config(format!("无法更新 {}: {error}", path.display())))
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

#[cfg(test)]
mod tests {
    use super::dotenv_error_detail;

    #[test]
    fn dotenv_parse_errors_do_not_expose_secret_lines() {
        let detail =
            dotenv_error_detail(dotenvy::Error::LineParse("YUANBAO_COOKIE=secret".into(), 7));
        assert_eq!(detail, "格式错误（位置 7）");
        assert!(!detail.contains("secret"));
    }
}
