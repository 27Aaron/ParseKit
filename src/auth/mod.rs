//! Shared credential vocabulary for platforms that need login.
//!
//! Resolvers still own how credentials are applied to HTTP requests.
//! This module only provides:
//! - a common [`CredentialStatus`] for CLI `doctor` / capability checks;
//! - cookie string helpers used by Bilibili today (WeChat can migrate later).

use std::{
    fs::OpenOptions,
    io::{self, Write},
    path::Path,
    sync::Arc,
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

/// Local assessment of configured credentials (no network verification).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialStatus {
    /// No credential is configured for this platform.
    Absent,
    /// A value is present but missing expected session markers.
    Incomplete,
    /// Local shape looks usable; upstream may still reject it.
    Present,
}

/// Opaque cookie header value (`name=value; name2=value2`).
///
/// Debug redacts the raw cookie so it never appears in logs by default.
#[derive(Clone)]
pub struct CookieCredential {
    raw: Arc<str>,
}

impl CookieCredential {
    /// Builds a credential from a cookie header string.
    ///
    /// Empty, whitespace-only, or invalid HTTP header input yields [`None`].
    pub fn new(cookie: impl Into<String>) -> Option<Self> {
        let cookie = cookie.into();
        let trimmed = cookie.trim();
        if trimmed.is_empty()
            || reqwest::header::HeaderValue::from_bytes(trimmed.as_bytes()).is_err()
        {
            return None;
        }
        Some(Self {
            raw: Arc::from(trimmed),
        })
    }

    /// Cookie header value for `Cookie:` requests.
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

impl std::fmt::Debug for CookieCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookieCredential")
            .field("raw", &"<redacted>")
            .finish()
    }
}

/// Returns the value of a single cookie name from a Cookie header string.
pub fn cookie_value(cookie: &str, name: &str) -> Option<String> {
    cookie.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then(|| value.to_owned())
    })
}

/// Converts a success-URL query string (`a=1&b=2`) into a Cookie header body.
///
/// Matches BBDown: replace `&` with `;` and escape commas in values.
pub fn query_string_to_cookie_header(query: &str) -> String {
    let query = query.strip_prefix('?').unwrap_or(query);
    let mut header = String::with_capacity(query.len());
    for part in query.split('&').filter(|part| !part.is_empty()) {
        if !header.is_empty() {
            header.push_str("; ");
        }
        for character in part.chars() {
            if character == ',' {
                header.push_str("%2C");
            } else {
                header.push(character);
            }
        }
    }
    header
}

/// Upserts `KEY=value` in a dotenv-style file (creates parent dirs / file as needed).
///
/// Used by CLI login to persist `BILIBILI_COOKIE` into `.env.local` without
/// rewriting an existing hand-edited `.env`.
pub fn upsert_dotenv_var(path: &Path, key: &str, value: &str) -> io::Result<()> {
    validate_dotenv_input(key, value)?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let exists = dotenv_path_exists_and_safe(path)?;
    let existing = if exists {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };

    let escaped = escape_dotenv_value(value);
    let line = format!("{key}={escaped}");
    let mut replaced = false;
    let mut out = String::new();
    for existing_line in existing.lines() {
        if line_assigns_key(existing_line, key) {
            // Collapse duplicate assignments so a later stale value cannot win.
            if !replaced {
                out.push_str(&line);
                out.push('\n');
                replaced = true;
            }
        } else {
            out.push_str(existing_line);
            out.push('\n');
        }
    }
    if !replaced {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&line);
        out.push('\n');
    }
    write_private_file(path, out.as_bytes())
}

/// Removes a key from a dotenv-style file if present.
pub fn remove_dotenv_var(path: &Path, key: &str) -> io::Result<bool> {
    validate_dotenv_key(key)?;
    if !dotenv_path_exists_and_safe(path)? {
        return Ok(false);
    }
    let existing = std::fs::read_to_string(path)?;
    let mut removed = false;
    let mut out = String::new();
    for existing_line in existing.lines() {
        if line_assigns_key(existing_line, key) {
            removed = true;
            continue;
        }
        out.push_str(existing_line);
        out.push('\n');
    }
    if removed {
        write_private_file(path, out.as_bytes())?;
    }
    Ok(removed)
}

fn escape_dotenv_value(value: &str) -> String {
    // Double-quoted form so `;`, spaces, and `=` inside cookies are preserved.
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '$' => escaped.push_str("\\$"),
            '\n' => escaped.push_str("\\n"),
            _ => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

fn line_assigns_key(line: &str, key: &str) -> bool {
    let mut assignment = line.trim_start();
    if assignment.starts_with('#') {
        return false;
    }
    if let Some(after_export) = assignment.strip_prefix("export")
        && after_export
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_whitespace)
    {
        assignment = after_export.trim_start();
    }
    assignment
        .split_once('=')
        .is_some_and(|(candidate, _)| candidate.trim_end() == key)
}

fn validate_dotenv_input(key: &str, value: &str) -> io::Result<()> {
    validate_dotenv_key(key)?;
    if value.bytes().any(|byte| matches!(byte, b'\0' | b'\r')) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "dotenv values cannot contain NUL or carriage-return characters",
        ));
    }
    Ok(())
}

fn validate_dotenv_key(key: &str) -> io::Result<()> {
    let mut bytes = key.bytes();
    let valid = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid dotenv variable name",
        ))
    }
}

fn dotenv_path_exists_and_safe(path: &Path) -> io::Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "dotenv path must be a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "dotenv path must not have multiple hard links",
            ));
        }
    }
    Ok(true)
}

fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true);
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);

    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "dotenv path must be a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "dotenv path must not have multiple hard links",
            ));
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.set_len(0)?;
    file.write_all(contents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cookie_value_reads_markers() {
        let cookie = "a=1; SESSDATA=abc%2Cdef; bili_jct=xyz";
        assert_eq!(
            cookie_value(cookie, "SESSDATA").as_deref(),
            Some("abc%2Cdef")
        );
        assert_eq!(cookie_value(cookie, "bili_jct").as_deref(), Some("xyz"));
        assert!(cookie_value(cookie, "missing").is_none());
        assert_eq!(
            cookie_value("token=a=b=c", "token").as_deref(),
            Some("a=b=c")
        );
        assert!(cookie_value("not_token=x", "token").is_none());
    }

    #[test]
    fn query_string_to_cookie_header_matches_bbdown_shape() {
        let query = "DedeUserID=1&SESSDATA=a,b&bili_jct=tok";
        assert_eq!(
            query_string_to_cookie_header(query),
            "DedeUserID=1; SESSDATA=a%2Cb; bili_jct=tok"
        );
    }

    #[test]
    fn upsert_and_remove_dotenv_var() {
        let dir = std::env::temp_dir().join(format!("parse-kit-auth-{}", uuid::Uuid::new_v4()));
        let path = dir.join(".env.local");
        fs::create_dir_all(&dir).unwrap();

        upsert_dotenv_var(&path, "BILIBILI_COOKIE", "SESSDATA=one").unwrap();
        let first = fs::read_to_string(&path).unwrap();
        assert!(first.contains("BILIBILI_COOKIE="));
        assert!(first.contains("SESSDATA=one"));

        upsert_dotenv_var(&path, "BILIBILI_COOKIE", "SESSDATA=two; bili_jct=x").unwrap();
        let second = fs::read_to_string(&path).unwrap();
        assert!(second.contains("SESSDATA=two"));
        assert_eq!(second.matches("BILIBILI_COOKIE=").count(), 1);

        assert!(remove_dotenv_var(&path, "BILIBILI_COOKIE").unwrap());
        let third = fs::read_to_string(&path).unwrap();
        assert!(!third.contains("BILIBILI_COOKIE"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dotenv_upsert_round_trips_special_characters_and_collapses_duplicates() {
        let dir = std::env::temp_dir().join(format!("parse-kit-auth-{}", uuid::Uuid::new_v4()));
        let path = dir.join(".env.local");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            &path,
            "export BILIBILI_COOKIE=old\n  BILIBILI_COOKIE = stale\n# BILIBILI_COOKIE=comment\n",
        )
        .unwrap();

        let expected = "SESSDATA=a$b; note=\"quoted\"; path=C:\\tmp\nnext";
        upsert_dotenv_var(&path, "BILIBILI_COOKIE", expected).unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(
            contents
                .lines()
                .filter(|line| line_assigns_key(line, "BILIBILI_COOKIE"))
                .count(),
            1
        );
        assert!(contents.contains("# BILIBILI_COOKIE=comment"));

        let parsed: Vec<_> = dotenvy::from_path_iter(&path)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect();
        assert_eq!(
            parsed
                .iter()
                .find(|(key, _)| key == "BILIBILI_COOKIE")
                .map(|(_, value)| value.as_str()),
            Some(expected)
        );

        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dotenv_helpers_reject_invalid_inputs() {
        let dir = std::env::temp_dir().join(format!("parse-kit-auth-{}", uuid::Uuid::new_v4()));
        let path = dir.join(".env.local");
        assert_eq!(
            upsert_dotenv_var(&path, "BAD\nKEY", "value")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            upsert_dotenv_var(&path, "GOOD_KEY", "bad\rvalue")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn dotenv_helpers_reject_links_without_touching_the_target() {
        let dir = std::env::temp_dir().join(format!("parse-kit-auth-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target");
        let link = dir.join(".env.local");
        fs::write(&target, "KEEP=original\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert_eq!(
            upsert_dotenv_var(&link, "BILIBILI_COOKIE", "SESSDATA=secret")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "KEEP=original\n");

        fs::remove_file(&link).unwrap();
        fs::hard_link(&target, &link).unwrap();
        assert_eq!(
            remove_dotenv_var(&link, "KEEP").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "KEEP=original\n");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cookie_credential_redacts_debug() {
        let cred = CookieCredential::new("SESSDATA=secret").unwrap();
        let debug = format!("{cred:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn cookie_credential_rejects_header_injection() {
        assert!(CookieCredential::new("SESSDATA=value\r\nX-Evil: injected").is_none());
    }
}
