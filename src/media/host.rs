//! Host validation shared by resolvers and media downloads.

use url::Url;

/// Matches an exact hostname or a dot-prefixed subdomain rule.
///
/// A rule such as `.cdn.example` matches `video.cdn.example`, but not the apex
/// `cdn.example` or a lookalike such as `evilcdn.example`.
pub(crate) fn host_matches_rules<I, S>(host: &str, rules: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    rules.into_iter().any(|rule| {
        let rule = rule.as_ref();
        if rule.starts_with('.') {
            host.len() > rule.len()
                && host
                    .get(host.len() - rule.len()..)
                    .is_some_and(|suffix| suffix.eq_ignore_ascii_case(rule))
        } else {
            host.eq_ignore_ascii_case(rule)
        }
    })
}

/// Returns `true` when a URL has a safe HTTPS authority and a reviewed host.
///
/// Non-443 HTTPS ports are allowed (CDN edges sometimes use them). DNS and IP
/// checks run separately immediately before download.
pub(crate) fn is_reviewed_https_url(url: &Url, rules: &[&str]) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && url.host_str().is_some_and(|host| {
            !host.ends_with('.') && host_matches_rules(host, rules.iter().copied())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_rules_require_a_domain_boundary_and_a_subdomain() {
        let rules = [".bilivideo.com", "media.example"];

        assert!(host_matches_rules("v1.bilivideo.com", rules));
        assert!(host_matches_rules("MEDIA.EXAMPLE", rules));
        assert!(!host_matches_rules("bilivideo.com", rules));
        assert!(!host_matches_rules("evilbilivideo.com", rules));
    }

    #[test]
    fn reviewed_urls_reject_unsafe_authority_components() {
        let rules = ["media.example"];
        for raw in [
            "http://media.example/video.mp4",
            "https://user@media.example/video.mp4",
            "https://media.example/video.mp4#fragment",
            "https://media.example./video.mp4",
            "https://media.example.evil.test/video.mp4",
        ] {
            let url = Url::parse(raw).expect("test URL");
            assert!(!is_reviewed_https_url(&url, &rules), "{raw}");
        }

        let url = Url::parse("https://media.example:443/video.mp4").expect("test URL");
        assert!(is_reviewed_https_url(&url, &rules));
        // CDN-style non-default HTTPS port on a reviewed host is accepted.
        let url = Url::parse("https://media.example:20443/video.mp4").expect("test URL");
        assert!(is_reviewed_https_url(&url, &rules));
    }
}
