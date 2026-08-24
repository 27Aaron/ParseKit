//! SM3, Simon, protobuf, and AES helpers matching SignerPy 0.12.0.

use aes::Aes128;
use cbc::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use md5::{Digest, Md5};

type Aes128CbcEnc = cbc::Encryptor<Aes128>;

pub(super) fn md5_hex(data: &[u8]) -> String {
    hex_lower(&Md5::digest(data))
}

pub(super) fn md5_digest(data: &[u8]) -> [u8; 16] {
    Md5::digest(data).into()
}

pub(super) fn aes128_cbc_encrypt(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let pad = 16 - (data.len() % 16);
    let mut buffer = data.to_vec();
    buffer.extend(std::iter::repeat_n(0, pad));
    let encrypted = Aes128CbcEnc::new(key.into(), iv.into())
        .encrypt_padded::<Pkcs7>(&mut buffer, data.len())
        .expect("PKCS7 padding fits reserved block");
    encrypted.to_vec()
}

pub(super) fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(super) fn hex_upper(bytes: &[u8]) -> String {
    hex_lower(bytes).to_ascii_uppercase()
}

pub(super) fn sm3_hash(msg: &[u8]) -> [u8; 32] {
    const IV: [u32; 8] = [
        1_937_774_191,
        1_226_093_241,
        388_252_375,
        3_666_478_592,
        2_842_636_476,
        372_324_522,
        3_817_729_613,
        2_969_243_214,
    ];
    const TJ0: u32 = 2_043_430_169;
    const TJ1: u32 = 2_055_708_042;

    let mut padded = msg.to_vec();
    let bit_length = (msg.len() as u64).saturating_mul(8);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_length.to_be_bytes());

    let mut state = IV;
    for chunk in padded.chunks_exact(64) {
        state = sm3_compress(state, chunk, TJ0, TJ1);
    }

    let mut out = [0u8; 32];
    for (index, word) in state.iter().enumerate() {
        out[index * 4..(index + 1) * 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn sm3_compress(v_i: [u32; 8], block: &[u8], tj0: u32, tj1: u32) -> [u32; 8] {
    let mut w = [0u32; 68];
    for (index, chunk) in block.chunks_exact(4).enumerate() {
        w[index] = u32::from_be_bytes(chunk.try_into().expect("4-byte SM3 word"));
    }
    for j in 16..68 {
        w[j] = p1(w[j - 16] ^ w[j - 9] ^ rotate_left(w[j - 3], 15))
            ^ rotate_left(w[j - 13], 7)
            ^ w[j - 6];
    }
    let mut w1 = [0u32; 64];
    for j in 0..64 {
        w1[j] = w[j] ^ w[j + 4];
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = v_i;
    for j in 0..64 {
        let tj = if j < 16 { tj0 } else { tj1 };
        let ss1 = rotate_left(
            rotate_left(a, 12)
                .wrapping_add(e)
                .wrapping_add(rotate_left(tj, j as u32)),
            7,
        );
        let ss2 = ss1 ^ rotate_left(a, 12);
        let tt1 = ffj(a, b, c, j)
            .wrapping_add(d)
            .wrapping_add(ss2)
            .wrapping_add(w1[j]);
        let tt2 = ggj(e, f, g, j)
            .wrapping_add(h)
            .wrapping_add(ss1)
            .wrapping_add(w[j]);
        d = c;
        c = rotate_left(b, 9);
        b = a;
        a = tt1;
        h = g;
        g = rotate_left(f, 19);
        f = e;
        e = p0(tt2);
    }

    [
        a ^ v_i[0],
        b ^ v_i[1],
        c ^ v_i[2],
        d ^ v_i[3],
        e ^ v_i[4],
        f ^ v_i[5],
        g ^ v_i[6],
        h ^ v_i[7],
    ]
}

fn rotate_left(value: u32, count: u32) -> u32 {
    value.rotate_left(count % 32)
}

fn ffj(x: u32, y: u32, z: u32, j: usize) -> u32 {
    if j < 16 {
        x ^ y ^ z
    } else {
        (x & y) | (x & z) | (y & z)
    }
}

fn ggj(x: u32, y: u32, z: u32, j: usize) -> u32 {
    if j < 16 {
        x ^ y ^ z
    } else {
        (x & y) | (!x & z)
    }
}

fn p0(x: u32) -> u32 {
    x ^ rotate_left(x, 9) ^ rotate_left(x, 17)
}

fn p1(x: u32) -> u32 {
    x ^ rotate_left(x, 15) ^ rotate_left(x, 23)
}

pub(super) fn simon_enc(pt: [u64; 2], key: [u64; 4]) -> [u64; 2] {
    let mut schedule = [0u64; 72];
    schedule[..4].copy_from_slice(&key);
    let magic: u64 = 0x3DC9_4C3A_046D_678B;
    for i in 4..72 {
        let mut tmp = schedule[i - 1].rotate_right(3);
        tmp ^= schedule[i - 3];
        tmp ^= tmp.rotate_right(1);
        let bit = (magic >> ((i - 4) % 62)) & 1;
        schedule[i] = (!schedule[i - 4]) ^ tmp ^ bit ^ 3;
    }

    let mut x_i = pt[0];
    let mut x_i1 = pt[1];
    for key_word in schedule {
        let tmp = x_i1;
        let f = x_i1.rotate_left(1) & x_i1.rotate_left(8);
        x_i1 = x_i ^ f ^ x_i1.rotate_left(2) ^ key_word;
        x_i = tmp;
    }
    [x_i, x_i1]
}

#[derive(Clone)]
pub(super) enum ProtoValue {
    Varint(u64),
    Bytes(Vec<u8>),
    Message(Vec<(u32, ProtoValue)>),
}

pub(super) fn encode_proto(fields: &[(u32, ProtoValue)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (idx, value) in fields {
        match value {
            ProtoValue::Varint(number) => {
                write_varint(&mut out, u64::from(*idx << 3));
                write_varint(&mut out, *number);
            }
            ProtoValue::Bytes(bytes) => {
                write_varint(&mut out, u64::from((*idx << 3) | 2));
                write_varint(&mut out, bytes.len() as u64);
                out.extend_from_slice(bytes);
            }
            ProtoValue::Message(nested) => {
                let encoded = encode_proto(nested);
                write_varint(&mut out, u64::from((*idx << 3) | 2));
                write_varint(&mut out, encoded.len() as u64);
                out.extend_from_slice(&encoded);
            }
        }
    }
    out
}

/// Matches SignerPy's 32-bit `writeVarint`, including `while vint > 0x80`.
fn write_varint(out: &mut Vec<u8>, value: u64) {
    let mut vint = value & 0xFFFF_FFFF;
    while vint > 0x80 {
        out.push(((vint & 0x7F) | 0x80) as u8);
        vint >>= 7;
    }
    out.push((vint & 0x7F) as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sm3_matches_official_abc_vector() {
        assert_eq!(
            hex_lower(&sm3_hash(b"abc")),
            "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0"
        );
    }

    #[test]
    fn sm3_matches_signerpy_zero_block() {
        assert_eq!(
            hex_lower(&sm3_hash(&[0; 16])),
            "106e34a2b8c7bb13156cfdd0d91379dcc47543dcf9787c68ae5eb582620ae6e8"
        );
    }

    #[test]
    fn proto_matches_signerpy_sample() {
        let encoded = encode_proto(&[
            (1, ProtoValue::Varint(2)),
            (4, ProtoValue::Bytes(b"1128".to_vec())),
            (5, ProtoValue::Bytes(b"111".to_vec())),
            (13, ProtoValue::Bytes(b"abcdef".to_vec())),
        ]);
        assert_eq!(
            hex_lower(&encoded),
            "08022204313132382a033131316a06616263646566"
        );
    }

    #[test]
    fn proto_varint_matches_signerpy_large_values() {
        let encoded = encode_proto(&[
            (1, ProtoValue::Varint(0x2020_0929 << 1)),
            (11, ProtoValue::Varint(0)),
            (12, ProtoValue::Varint(1_700_000_000 << 1)),
        ]);
        assert_eq!(hex_lower(&encoded), "08d2a480820458006080c49fd50c");
    }
}
