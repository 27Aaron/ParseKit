use std::{io::SeekFrom, path::Path};

use tokio::{
    fs::OpenOptions,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

use crate::{Error, Result, media::bmff::looks_like_bmff};

pub(crate) const ENCRYPTED_PREFIX_BYTES: usize = 128 * 1024;

/// Decrypts a WeChat prefix and reports whether the file changed.
pub async fn decrypt_file_prefix(path: &Path, decode_key: u64) -> Result<bool> {
    let mut file = OpenOptions::new().read(true).write(true).open(path).await?;
    let length = usize::try_from(
        file.metadata()
            .await?
            .len()
            .min(u64::try_from(ENCRYPTED_PREFIX_BYTES).expect("encrypted prefix limit fits u64")),
    )
    .expect("encrypted prefix length fits usize");
    if length < 8 {
        return Err(Error::InvalidMedia("文件短于 MP4 文件头".into()));
    }

    let mut prefix = vec![0_u8; length];
    file.read_exact(&mut prefix).await?;
    if looks_like_bmff(&prefix) {
        return Ok(false);
    }

    let mut xor = PrefixXor::new(decode_key);
    for chunk in prefix.chunks_mut(8 * 1024) {
        xor.transform(chunk);
    }
    if !looks_like_bmff(&prefix) {
        return Err(Error::InvalidMedia(
            "decodeKey 与媒体不匹配，解密后没有有效 BMFF 文件头".into(),
        ));
    }

    file.seek(SeekFrom::Start(0)).await?;
    file.write_all(&prefix).await?;
    file.flush().await?;
    Ok(true)
}

/// Applies the WeChat XOR keystream to the encrypted prefix.
pub(crate) struct PrefixXor {
    isaac: Isaac64,
    block: [u8; 8],
    block_pos: usize,
    pub(crate) remaining: usize,
}

impl PrefixXor {
    pub(crate) fn new(key: u64) -> Self {
        Self {
            isaac: Isaac64::new(key),
            block: [0; 8],
            block_pos: 8,
            remaining: ENCRYPTED_PREFIX_BYTES,
        }
    }

    pub(crate) fn transform(&mut self, data: &mut [u8]) {
        let limit = data.len().min(self.remaining);
        let mut offset = 0;
        while offset < limit {
            if self.block_pos >= 8 {
                self.block = self.isaac.next().to_be_bytes();
                self.block_pos = 0;
            }
            let count = (8 - self.block_pos).min(limit - offset);
            for index in 0..count {
                data[offset + index] ^= self.block[self.block_pos + index];
            }
            offset += count;
            self.block_pos += count;
        }
        self.remaining -= limit;
    }
}

#[cfg(test)]
fn xor_keystream(data: &mut [u8], key: u64) {
    let mut xor = PrefixXor::new(key);
    xor.transform(data);
}

struct Isaac64 {
    count: usize,
    seed: [u64; 256],
    memory: [u64; 256],
    aa: u64,
    bb: u64,
    cc: u64,
}

impl Isaac64 {
    fn new(key: u64) -> Self {
        let mut state = Self {
            count: 255,
            seed: [0; 256],
            memory: [0; 256],
            aa: 0,
            bb: 0,
            cc: 0,
        };
        state.seed[0] = key;
        state.initialize();
        state
    }

    fn next(&mut self) -> u64 {
        let result = self.seed[self.count];
        if self.count == 0 {
            self.generate();
            self.count = 255;
        } else {
            self.count -= 1;
        }
        result
    }

    fn initialize(&mut self) {
        const GOLDEN: u64 = 0x9e37_79b9_7f4a_7c13;
        let mut values = [GOLDEN; 8];
        for _ in 0..4 {
            mix(&mut values);
        }

        for start in (0..256).step_by(8) {
            for (offset, value) in values.iter_mut().enumerate() {
                *value = value.wrapping_add(self.seed[start + offset]);
            }
            mix(&mut values);
            self.memory[start..start + 8].copy_from_slice(&values);
        }

        for start in (0..256).step_by(8) {
            for (offset, value) in values.iter_mut().enumerate() {
                *value = value.wrapping_add(self.memory[start + offset]);
            }
            mix(&mut values);
            self.memory[start..start + 8].copy_from_slice(&values);
        }

        self.generate();
    }

    fn generate(&mut self) {
        self.cc = self.cc.wrapping_add(1);
        self.bb = self.bb.wrapping_add(self.cc);

        for index in 0..256 {
            self.aa = match index % 4 {
                0 => !(self.aa ^ self.aa.wrapping_shl(21)),
                1 => self.aa ^ (self.aa >> 5),
                2 => self.aa ^ self.aa.wrapping_shl(12),
                _ => self.aa ^ (self.aa >> 33),
            };
            self.aa = self.aa.wrapping_add(self.memory[(index + 128) % 256]);

            let x = self.memory[index];
            let y = self.memory[((x >> 3) & 255) as usize]
                .wrapping_add(self.aa)
                .wrapping_add(self.bb);
            self.memory[index] = y;
            self.bb = self.memory[((y >> 11) & 255) as usize].wrapping_add(x);
            self.seed[index] = self.bb;
        }
    }
}

fn mix(values: &mut [u64; 8]) {
    values[0] = values[0].wrapping_sub(values[4]);
    values[5] ^= values[7] >> 9;
    values[7] = values[7].wrapping_add(values[0]);
    values[1] = values[1].wrapping_sub(values[5]);
    values[6] ^= values[0].wrapping_shl(9);
    values[0] = values[0].wrapping_add(values[1]);
    values[2] = values[2].wrapping_sub(values[6]);
    values[7] ^= values[1] >> 23;
    values[1] = values[1].wrapping_add(values[2]);
    values[3] = values[3].wrapping_sub(values[7]);
    values[0] ^= values[2].wrapping_shl(15);
    values[2] = values[2].wrapping_add(values[3]);
    values[4] = values[4].wrapping_sub(values[0]);
    values[1] ^= values[3] >> 14;
    values[3] = values[3].wrapping_add(values[4]);
    values[5] = values[5].wrapping_sub(values[1]);
    values[2] ^= values[4].wrapping_shl(20);
    values[4] = values[4].wrapping_add(values[5]);
    values[6] = values[6].wrapping_sub(values[2]);
    values[3] ^= values[5] >> 17;
    values[5] = values[5].wrapping_add(values[6]);
    values[7] = values[7].wrapping_sub(values[3]);
    values[4] ^= values[6].wrapping_shl(14);
    values[6] = values[6].wrapping_add(values[7]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use uuid::Uuid;

    #[test]
    fn decrypts_known_wechat_prefix() {
        let mut encrypted = hex::decode("23766a16ff8ffe1a1ca6cd5f994846ab").unwrap();
        xor_keystream(&mut encrypted, 2_136_343_393);
        assert_eq!(hex::encode(encrypted), "000000206674797069736f6d00000200");
    }

    #[test]
    fn isaac64_matches_full_reference_keystream() {
        let mut keystream = vec![0_u8; ENCRYPTED_PREFIX_BYTES];
        xor_keystream(&mut keystream, 2_136_343_393);
        assert_eq!(
            hex::encode(Sha256::digest(&keystream)),
            "49b96d6fc75ba5215fbb773ce98f6b20f6441a7ac40abc9582b1e42c5f3cd9d8"
        );
        assert_eq!(
            hex::encode(&keystream[2040..2056]),
            "d4dd47152c315d9b07cd2de253c1704f"
        );
        assert_eq!(
            hex::encode(&keystream[ENCRYPTED_PREFIX_BYTES - 16..]),
            "ec42b0626a79c34877717be3fe41f933"
        );
    }

    #[tokio::test]
    async fn decrypts_only_prefix_and_does_not_commit_a_wrong_key() {
        let path = std::env::temp_dir().join(format!(
            "parse-kit-decrypt-test-{}.mp4",
            Uuid::new_v4().simple()
        ));
        let mut plaintext = vec![0_u8; ENCRYPTED_PREFIX_BYTES + 32];
        plaintext[..4].copy_from_slice(&32_u32.to_be_bytes());
        plaintext[4..8].copy_from_slice(b"ftyp");
        plaintext[ENCRYPTED_PREFIX_BYTES..].fill(0xa5);

        let mut encrypted = plaintext.clone();
        xor_keystream(&mut encrypted[..ENCRYPTED_PREFIX_BYTES], 2_136_343_393);
        tokio::fs::write(&path, &encrypted).await.unwrap();

        assert!(decrypt_file_prefix(&path, 123).await.is_err());
        assert_eq!(tokio::fs::read(&path).await.unwrap(), encrypted);

        assert!(decrypt_file_prefix(&path, 2_136_343_393).await.unwrap());
        assert_eq!(tokio::fs::read(&path).await.unwrap(), plaintext);
        assert!(!decrypt_file_prefix(&path, 2_136_343_393).await.unwrap());
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[test]
    fn streaming_xor_matches_bulk_keystream() {
        let mut bulk = vec![0x5a_u8; ENCRYPTED_PREFIX_BYTES + 64];
        let mut stream = bulk.clone();
        xor_keystream(&mut bulk[..ENCRYPTED_PREFIX_BYTES], 2_136_343_393);

        let mut xor = PrefixXor::new(2_136_343_393);
        for chunk in stream.chunks_mut(137) {
            xor.transform(chunk);
        }
        assert_eq!(xor.remaining, 0);
        assert_eq!(
            &stream[..ENCRYPTED_PREFIX_BYTES],
            &bulk[..ENCRYPTED_PREFIX_BYTES]
        );
        assert_eq!(
            &stream[ENCRYPTED_PREFIX_BYTES..],
            &bulk[ENCRYPTED_PREFIX_BYTES..]
        );
    }

    #[test]
    fn detects_iso_base_media_boxes() {
        assert!(looks_like_bmff(b"\0\0\0\x20ftypisom"));
        let mut prefixed = vec![0_u8; 4_112];
        prefixed[..4].copy_from_slice(&4104_u32.to_be_bytes());
        prefixed[4..8].copy_from_slice(b"free");
        prefixed[4104..4108].copy_from_slice(&8_u32.to_be_bytes());
        prefixed[4108..4112].copy_from_slice(b"ftyp");
        assert!(looks_like_bmff(&prefixed));
        assert!(!looks_like_bmff(b"<html>not a video</html>"));
        assert!(!looks_like_bmff(b"random text mdat random text"));
    }
}
