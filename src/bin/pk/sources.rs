//! Media source selection for download.

use parse_kit::{Error, MediaSource, ResolvedPost, Result};

use crate::args::Prefer;

pub fn select_sources(
    post: &ResolvedPost,
    prefer: Prefer,
    source: Option<usize>,
) -> Result<Vec<&MediaSource>> {
    let mut sources: Vec<&MediaSource> = post.media_sources().collect();
    if sources.is_empty() {
        return Err(Error::MediaUnavailable);
    }

    match prefer {
        Prefer::Best => {}
        Prefer::Smallest => sources.reverse(),
    }

    if let Some(index) = source {
        let chosen = sources.get(index).copied().ok_or_else(|| {
            Error::Config(format!(
                "无效的 --source {index}（可用 0..{}）",
                sources.len().saturating_sub(1)
            ))
        })?;
        return Ok(vec![chosen]);
    }

    Ok(sources)
}
