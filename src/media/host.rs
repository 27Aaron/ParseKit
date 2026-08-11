//! Host validation shared by resolvers and media downloads.

use url::Url;

/// Matches exact hosts or dot-prefixed subdomain rules with label boundaries.
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

/// Checks the HTTPS authority and reviewed host; DNS checks run at download time.
pub(crate) fn is_reviewed_https_url(url: &Url, rules: &[&str]) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && url.port() != Some(0)
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
            "https://media.example:0/video.mp4",
        ] {
            let url = Url::parse(raw).expect("test URL");
            assert!(!is_reviewed_https_url(&url, &rules), "{raw}");
        }

        let url = Url::parse("https://media.example:443/video.mp4").expect("test URL");
        assert!(is_reviewed_https_url(&url, &rules));
        let url = Url::parse("https://media.example:20443/video.mp4").expect("test URL");
        assert!(is_reviewed_https_url(&url, &rules));
    }
}
