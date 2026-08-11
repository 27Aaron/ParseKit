//! Shared credential vocabulary for platforms that need login.
//!
//! Resolvers still own how credentials are applied to HTTP requests.
//! This module only provides:
//! - a common [`CredentialStatus`] for CLI `doctor` / capability checks;
//! - cookie string helpers used by Bilibili today (WeChat can migrate later).

use std::sync::Arc;

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
    /// Empty / whitespace-only input yields [`None`].
    pub fn new(cookie: impl Into<String>) -> Option<Self> {
        let cookie = cookie.into();
        let trimmed = cookie.trim();
        if trimmed.is_empty() {
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
    let prefix = format!("{name}=");
    cookie.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix(&prefix).map(str::to_owned)
    })
}

/// Converts a success-URL query string (`a=1&b=2`) into a Cookie header body.
///
/// Matches BBDown: replace `&` with `;` and escape commas in values.
pub fn query_string_to_cookie_header(query: &str) -> String {
    let query = query.strip_prefix('?').unwrap_or(query);
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| part.replace(',', "%2C"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Upserts `KEY=value` in a dotenv-style file (creates parent dirs / file as needed).
///
/// Used by CLI login to persist `BILIBILI_COOKIE` into `.env.local` without
/// rewriting an existing hand-edited `.env`.
pub fn upsert_dotenv_var(path: &std::path::Path, key: &str, value: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let existing = if path.exists() {
        std::fs::read_to_string(path)?
    } else {
        String::new()
    };

    let escaped = escape_dotenv_value(value);
    let line = format!("{key}={escaped}");
    let prefix = format!("{key}=");
    let mut replaced = false;
    let mut out = String::new();
    for existing_line in existing.lines() {
        let trimmed = existing_line.trim_start();
        if trimmed.starts_with(&prefix) && !trimmed.starts_with('#') {
            out.push_str(&line);
            out.push('\n');
            replaced = true;
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
    std::fs::write(path, out)
}

/// Removes a key from a dotenv-style file if present.
pub fn remove_dotenv_var(path: &std::path::Path, key: &str) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let existing = std::fs::read_to_string(path)?;
    let prefix = format!("{key}=");
    let mut removed = false;
    let mut out = String::new();
    for existing_line in existing.lines() {
        let trimmed = existing_line.trim_start();
        if trimmed.starts_with(&prefix) && !trimmed.starts_with('#') {
            removed = true;
            continue;
        }
        out.push_str(existing_line);
        out.push('\n');
    }
    if removed {
        std::fs::write(path, out)?;
    }
    Ok(removed)
}

fn escape_dotenv_value(value: &str) -> String {
    // Double-quoted form so `;`, spaces, and `=` inside cookies are preserved.
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
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
    fn cookie_credential_redacts_debug() {
        let cred = CookieCredential::new("SESSDATA=secret").unwrap();
        let debug = format!("{cred:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("secret"));
    }
}
