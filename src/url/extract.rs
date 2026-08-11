//! Extract URLs from free-form share text.

/// Punctuation commonly appended to URLs in chat messages.
pub const URL_TRAILING_PUNCT: &[char] = &[
    '。', '，', ',', '.', '！', '!', '？', '?', ')', '）', ']', '】', '、',
];

pub fn trim_url_candidate(matched: &str) -> &str {
    matched.trim_end_matches(URL_TRAILING_PUNCT)
}

/// Returns the first HTTPS URL after trimming trailing chat punctuation.
pub fn first_https_url(input: &str) -> Option<&str> {
    let start = input.find("https://")?;
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
    }

    #[test]
    fn finds_first_https_url_in_share_text() {
        assert_eq!(
            first_https_url("看看 https://www.bilibili.com/video/BV1GJ411x7h7 这个"),
            Some("https://www.bilibili.com/video/BV1GJ411x7h7")
        );
        assert_eq!(first_https_url("no link here"), None);
    }
}
