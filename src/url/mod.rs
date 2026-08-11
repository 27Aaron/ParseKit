//! URL extraction and normalization utilities.

mod clean;
mod extract;

pub use clean::{CleanPolicy, clean_tracking_params, strip_fragment};
pub use extract::{URL_TRAILING_PUNCT, first_https_url, trim_url_candidate};
