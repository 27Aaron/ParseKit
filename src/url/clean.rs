//! Remove tracking data while preserving required URL parameters.

use url::Url;

/// Defines the query parameters preserved during URL cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CleanPolicy {
    /// Query parameter names to preserve, matched case-insensitively.
    pub reserved: &'static [&'static str],
}

impl CleanPolicy {
    pub const EMPTY: Self = Self { reserved: &[] };

    /// Preserves parameters required by signed media URLs.
    pub const MEDIA_SIGNED: Self = Self {
        reserved: &["token", "encfilekey", "decodekey"],
    };

    /// Preserves structural parameters used by share pages.
    pub const SHARE_PAGE: Self = Self {
        reserved: &["p", "t", "spm_id_from"],
    };
}

/// Removes known tracking parameters and fragments while preserving reserved keys.
pub fn clean_tracking_params(url: &Url, policy: CleanPolicy) -> Url {
    let mut cleaned = url.clone();
    cleaned.set_fragment(None);

    if url.query().is_some() {
        let mut pairs = url.query_pairs().filter(|(key, _)| {
            policy
                .reserved
                .iter()
                .any(|reserved| key.eq_ignore_ascii_case(reserved))
                || !is_tracking_key(key.as_ref())
        });

        cleaned.set_query(None);
        if let Some((key, value)) = pairs.next() {
            let mut serializer = cleaned.query_pairs_mut();
            serializer.append_pair(&key, &value);
            for (key, value) in pairs {
                serializer.append_pair(&key, &value);
            }
        }
    }
    cleaned
}

pub fn strip_fragment(url: &Url) -> Url {
    let mut cleaned = url.clone();
    cleaned.set_fragment(None);
    cleaned
}

fn is_tracking_key(key: &str) -> bool {
    const EXACT_TRACKING_KEYS: &[&str] = &[
        "from",
        "isappinstalled",
        "scene",
        "share_source",
        "share_medium",
        "share_plat",
        "share_tag",
        "share_session_id",
        "tt_from",
        "u_code",
        "timestamp",
        "mid",
        "vd_source",
        "feature",
        "refer",
        "referer",
        "source",
        "ft",
        "unique_k",
    ];

    key.as_bytes()
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"utm_"))
        || key
            .as_bytes()
            .get(..3)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"spm"))
        || EXACT_TRACKING_KEYS
            .iter()
            .any(|tracking| key.eq_ignore_ascii_case(tracking))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_utm_and_keeps_reserved() {
        let raw = Url::parse(
            "https://finder.video.qq.com/v.mp4?utm_source=x&token=t&encfilekey=k&from=share",
        )
        .unwrap();
        let cleaned = clean_tracking_params(&raw, CleanPolicy::MEDIA_SIGNED);
        assert_eq!(cleaned.query(), Some("token=t&encfilekey=k"));
        assert!(cleaned.fragment().is_none());
    }

    #[test]
    fn share_page_drops_trackers() {
        let raw = Url::parse(
            "https://www.bilibili.com/video/BV1GJ411x7h7?utm_source=copy&spm_id_from=333",
        )
        .unwrap();
        let cleaned = clean_tracking_params(&raw, CleanPolicy::SHARE_PAGE);
        assert_eq!(cleaned.query(), Some("spm_id_from=333"));
    }

    #[test]
    fn matching_is_case_insensitive_and_preserves_pair_order() {
        let raw = Url::parse(
            "https://example.com/watch?keep=1&UTM_Source=copy&TOKEN=a&TOKEN=b&SpM=x#details",
        )
        .unwrap();
        let cleaned = clean_tracking_params(&raw, CleanPolicy::MEDIA_SIGNED);

        assert_eq!(cleaned.query(), Some("keep=1&TOKEN=a&TOKEN=b"));
        assert!(cleaned.fragment().is_none());
    }

    #[test]
    fn empty_result_removes_the_query_delimiter() {
        let raw = Url::parse("https://example.com/watch?utm_source=copy#details").unwrap();
        let cleaned = clean_tracking_params(&raw, CleanPolicy::EMPTY);

        assert_eq!(cleaned.as_str(), "https://example.com/watch");
    }
}
