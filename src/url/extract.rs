//! Extract URLs from free-form share text.

/// Punctuation commonly appended to URLs in chat messages.
pub const URL_TRAILING_PUNCT: &[char] = &[
    '。', '，', ',', '.', '！', '!', '？', '?', ')', '）', ']', '】', '、',
];

pub fn trim_url_candidate(matched: &str) -> &str {
    matched.trim_end_matches(URL_TRAILING_PUNCT)
}

/// Returns the first HTTPS URL after trimming trailing chat punctuation.
/// Scheme matching is ASCII-case-insensitive, as required by URL syntax.
pub fn first_https_url(input: &str) -> Option<&str> {
    const HTTPS_PREFIX: &[u8] = b"https://";
    let lowercase_start = input.find("https://");
    let prefix_end = lowercase_start.unwrap_or(input.len());
    let start = input
        .as_bytes()
        .get(..prefix_end)
        .and_then(|prefix| {
            prefix
                .windows(HTTPS_PREFIX.len())
                .position(|window| window.eq_ignore_ascii_case(HTTPS_PREFIX))
        })
        .or(lowercase_start)?;
    let rest = &input[start..];
    // Whitespace delimits the candidate; punctuation is trimmed separately so
    // dots within hostnames such as `b23.tv` remain intact.
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let candidate = trim_url_candidate(&rest[..end]);
    (!candidate.is_empty()).then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_common_chat_punctuation() {
        assert_eq!(
            trim_url_candidate("https://v.douyin.com/q75E3VmAe6A/。"),
            "https://v.douyin.com/q75E3VmAe6A/"
        );
        assert_eq!(
            trim_url_candidate("https://weixin.qq.com/sph/AzJ7CGPYWD】"),
            "https://weixin.qq.com/sph/AzJ7CGPYWD"
        );
        assert_eq!(
            trim_url_candidate("https://cdn.example.com/path.with.dots/file.mp4"),
            "https://cdn.example.com/path.with.dots/file.mp4"
        );
    }

    #[test]
    fn finds_first_https_url_in_share_text() {
        assert_eq!(
            first_https_url("看看 https://www.bilibili.com/video/BV1GJ411x7h7 这个"),
            Some("https://www.bilibili.com/video/BV1GJ411x7h7")
        );
        assert_eq!(first_https_url("no link here"), None);
    }

    #[test]
    fn finds_scheme_case_insensitively() {
        assert_eq!(
            first_https_url("first HTTPS://example.com/upper second https://example.com/lower"),
            Some("HTTPS://example.com/upper")
        );
    }
}
