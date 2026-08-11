//! ISO Base Media File Format (BMFF) prefix validation.

use std::path::Path;

use tokio::{fs::OpenOptions, io::AsyncReadExt};

use crate::Result;

/// Checks whether `data` starts with a valid BMFF box sequence.
pub fn looks_like_bmff(data: &[u8]) -> bool {
    let mut offset = 0_usize;
    for _ in 0..16 {
        let Some(header) = data.get(offset..offset.saturating_add(8)) else {
            return false;
        };
        let short_size = u32::from_be_bytes(header[..4].try_into().expect("four-byte slice"));
        let box_type = &header[4..8];
        let (size, header_size) = if short_size == 1 {
            let Some(extended) = data.get(offset + 8..offset + 16) else {
                return false;
            };
            (
                u64::from_be_bytes(extended.try_into().expect("eight-byte slice")),
                16_u64,
            )
        } else if short_size == 0 {
            (data.len().saturating_sub(offset) as u64, 8_u64)
        } else {
            (u64::from(short_size), 8_u64)
        };
        if size < header_size {
            return false;
        }
        if matches!(box_type, b"ftyp" | b"styp" | b"moov" | b"mdat") {
            return true;
        }
        let Ok(size) = usize::try_from(size) else {
            return false;
        };
        let Some(next) = offset.checked_add(size) else {
            return false;
        };
        if next <= offset || next > data.len() {
            return false;
        }
        offset = next;
    }
    false
}

/// Checks at most `max_bytes` of a file for BMFF boxes.
pub async fn prefix_looks_like_bmff(path: &Path, max_bytes: usize) -> Result<bool> {
    let file = OpenOptions::new().read(true).open(path).await?;
    let max_bytes = u64::try_from(max_bytes).unwrap_or(u64::MAX);
    let length = usize::try_from(file.metadata().await?.len().min(max_bytes))
        .expect("length is bounded by max_bytes");
    if length < 8 {
        return Ok(false);
    }
    let mut prefix = Vec::with_capacity(length);
    // A limited read tolerates concurrent truncation after metadata lookup.
    file.take(u64::try_from(length).expect("length fits u64"))
        .read_to_end(&mut prefix)
        .await?;
    Ok(looks_like_bmff(&prefix))
}

/// Checks the standard 128 KiB prefix for BMFF boxes.
pub async fn file_prefix_looks_like_bmff(path: &Path) -> Result<bool> {
    prefix_looks_like_bmff(path, 128 * 1024).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_minimal_ftyp() {
        let mut data = vec![0_u8; 24];
        data[0..4].copy_from_slice(&24_u32.to_be_bytes());
        data[4..8].copy_from_slice(b"ftyp");
        data[8..12].copy_from_slice(b"isom");
        assert!(looks_like_bmff(&data));
        assert!(!looks_like_bmff(b"not a media header!!!"));
    }

    #[test]
    fn skips_well_formed_prefix_boxes() {
        let mut data = Vec::new();
        data.extend_from_slice(&8_u32.to_be_bytes());
        data.extend_from_slice(b"free");
        data.extend_from_slice(&16_u32.to_be_bytes());
        data.extend_from_slice(b"ftyp");
        data.extend_from_slice(b"isom\0\0\0\0");

        assert!(looks_like_bmff(&data));
    }

    #[test]
    fn rejects_invalid_box_sizes() {
        let mut too_small = Vec::new();
        too_small.extend_from_slice(&4_u32.to_be_bytes());
        too_small.extend_from_slice(b"free");
        assert!(!looks_like_bmff(&too_small));

        let mut truncated_extended = Vec::new();
        truncated_extended.extend_from_slice(&1_u32.to_be_bytes());
        truncated_extended.extend_from_slice(b"free");
        assert!(!looks_like_bmff(&truncated_extended));
    }
}
