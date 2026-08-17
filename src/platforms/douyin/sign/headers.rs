//! Gorgon / Ladon / Argus header generation matching SignerPy 0.12.0.

use std::time::{SystemTime, UNIX_EPOCH};

use super::crypto::{
    ProtoValue, aes128_cbc_encrypt, encode_proto, hex_lower, hex_upper, md5_digest, md5_hex,
    simon_enc, sm3_hash,
};

const DEFAULT_SDK_VERSION_STR: &str = "v05.01.02-alpha.7-ov-android";
const DEFAULT_SDK_VERSION: u32 = 83_952_160;
const ARGUS_SIGN_KEY: &[u8] = b"\xac\x1a\xda\xae\x95\xa7\xaf\x94\xa5\x11J\xb3\xb3\xa9}\xd8\x00P\xaa\n91L@R\x8c\xae\xc9RV\xc2\x8c";
const ARGUS_SM3_OUTPUT: &[u8] =
    b"\xfcx\xe0\xa9ez\x0ct\x8c\xe5\x15Y\x90<\xcf\x03Q\x0eQ\xd3\xcf\xf22\xd7\x13C\xe8\x8a2\x1cS\x04";

#[derive(Debug, Clone)]
pub struct SignedHeaders {
    pub x_ss_req_ticket: String,
    pub x_khronos: String,
    pub x_gorgon: String,
    pub x_ss_stub: String,
    pub x_ladon: String,
    pub x_argus: String,
}

pub fn sign_query(query: &str, aid: u32, license_id: u32, gorgon_version: u16) -> SignedHeaders {
    sign_query_at(
        query,
        "",
        now_unix(),
        aid,
        license_id,
        gorgon_version,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn sign_query_at(
    query: &str,
    payload: &str,
    unix: u64,
    aid: u32,
    license_id: u32,
    gorgon_version: u16,
    sec_device_id: Option<&str>,
    argus_rand: Option<u32>,
) -> SignedHeaders {
    let gorgon = gorgon(query, payload, "", unix, gorgon_version);
    let stub = md5_hex(payload.as_bytes());
    SignedHeaders {
        x_ss_req_ticket: gorgon.0,
        x_khronos: gorgon.1,
        x_gorgon: gorgon.2,
        x_ss_stub: hex_upper(&md5_digest(payload.as_bytes())),
        x_ladon: ladon_encrypt(unix, license_id, aid, None),
        x_argus: argus_sign(
            query,
            &stub,
            unix,
            aid,
            license_id,
            0,
            sec_device_id.unwrap_or(""),
            DEFAULT_SDK_VERSION_STR,
            DEFAULT_SDK_VERSION,
            argus_rand,
        ),
    }
}

pub fn trace_id(device_id: &str) -> String {
    let millis = now_millis();
    let e = format!("{:08x}", millis % 4_294_967_295);
    let cleaned = device_id.replace('-', "");
    let numeric = cleaned.parse::<u128>().unwrap_or(0);
    let e2 = format!("{numeric:x}");
    let r = 22usize.saturating_sub(e2.len()).saturating_sub(4);
    let c_len = format!("{:02}", e2.len());
    let seed_source = format!("{:x}", random_u64().saturating_mul(10).saturating_pow(6));
    let seed: String = seed_source.chars().take(r).collect();
    let body = format!("{e}{c_len}{e2}{seed}");
    let prefix: String = body.chars().take(16).collect();
    format!("00-{body}-{prefix}-01")
}

fn gorgon(
    params: &str,
    payload: &str,
    cookie: &str,
    unix: u64,
    version: u16,
) -> (String, String, String) {
    match version {
        8404 => gorgon_v1(params, payload, cookie, unix),
        8402 => gorgon_v2(params, payload, cookie, unix, "840280416000"),
        _ => gorgon_v2(params, payload, cookie, unix, "0404b0d30000"),
    }
}

fn gorgon_v1(params: &str, payload: &str, cookie: &str, unix: u64) -> (String, String, String) {
    let mut gorgon = md5_prefix_bytes(params);
    append_optional_md5(&mut gorgon, payload);
    append_optional_md5(&mut gorgon, cookie);
    gorgon.extend_from_slice(&[0x01, 0x01, 0x02, 0x04]);
    let khronos = format!("{unix:08x}");
    for index in 0..4 {
        gorgon.push(u8::from_str_radix(&khronos[index * 2..index * 2 + 2], 16).unwrap_or(0));
    }
    let random_a = random_u8();
    let random_b = random_u8() & 0xf0;
    let mut encoded = String::from("8404");
    encoded.push_str(&format!("{random_b:02x}{random_a:02x}0000"));
    encoded.push_str(&hex_lower(&gorgon));
    (
        (unix.saturating_mul(1000)).to_string(),
        unix.to_string(),
        encoded,
    )
}

fn gorgon_v2(
    params: &str,
    payload: &str,
    cookie: &str,
    unix: u64,
    prefix: &str,
) -> (String, String, String) {
    let mut param_list = md5_prefix_bytes(params);
    append_optional_md5(&mut param_list, payload);
    append_optional_md5(&mut param_list, cookie);
    param_list.extend_from_slice(&[0x00, 0x06, 0x0B, 0x1C]);
    let khronos = (unix & 0xFFFF_FFFF) as u32;
    param_list.extend_from_slice(&khronos.to_be_bytes());

    const KEY: [u8; 20] = [
        0xDF, 0x77, 0xB9, 0x40, 0xB9, 0x9B, 0x84, 0x83, 0xD1, 0xB9, 0xCB, 0xD1, 0xF7, 0xC2, 0xB9,
        0x85, 0xC3, 0xD0, 0xFB, 0xC3,
    ];
    let mut eor: Vec<u8> = param_list
        .iter()
        .zip(KEY)
        .map(|(left, right)| left ^ right)
        .collect();
    let length = 0x14u8;
    for i in 0..eor.len() {
        let reversed = reverse_nibble(eor[i]);
        let next = eor[(i + 1) % eor.len()];
        let mixed = reversed ^ next;
        eor[i] = ((!u32::from(rbit(mixed))) ^ u32::from(length)) as u8;
    }
    (
        (unix.saturating_mul(1000)).to_string(),
        unix.to_string(),
        format!("{prefix}{}", hex_lower(&eor)),
    )
}

fn md5_prefix_bytes(input: &str) -> Vec<u8> {
    hex_to_prefix_bytes(&md5_hex(input.as_bytes()))
}

fn append_optional_md5(out: &mut Vec<u8>, input: &str) {
    if input.is_empty() {
        out.extend_from_slice(&[0, 0, 0, 0]);
    } else {
        out.extend_from_slice(&hex_to_prefix_bytes(&md5_hex(input.as_bytes())));
    }
}

fn hex_to_prefix_bytes(hex: &str) -> Vec<u8> {
    (0..4)
        .map(|index| u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap_or(0))
        .collect()
}

fn reverse_nibble(num: u8) -> u8 {
    let hex = format!("{num:02x}");
    u8::from_str_radix(&format!("{}{}", &hex[1..], &hex[..1]), 16).unwrap_or(num)
}

fn rbit(num: u8) -> u8 {
    num.reverse_bits()
}

pub(super) fn ladon_encrypt(
    khronos: u64,
    license_id: u32,
    aid: u32,
    random_bytes: Option<[u8; 4]>,
) -> String {
    let random_bytes = random_bytes.unwrap_or_else(random_4);
    let data = format!("{khronos}-{license_id}-{aid}");
    let mut keygen = random_bytes.to_vec();
    keygen.extend_from_slice(aid.to_string().as_bytes());
    let md5hex = md5_hex(&keygen);
    let encrypted = encrypt_ladon(md5hex.as_bytes(), data.as_bytes());
    let mut output = Vec::with_capacity(4 + encrypted.len());
    output.extend_from_slice(&random_bytes);
    output.extend_from_slice(&encrypted);
    base64_encode(&output)
}

fn encrypt_ladon(md5hex: &[u8], data: &[u8]) -> Vec<u8> {
    let mut hash_table = vec![0u8; 272 + 16];
    hash_table[..32.min(md5hex.len())].copy_from_slice(&md5hex[..32.min(md5hex.len())]);

    let mut temp = Vec::with_capacity(4);
    for i in 0..4 {
        temp.push(u64_le(&hash_table[i * 8..(i + 1) * 8]));
    }
    let mut buffer_b0 = temp.remove(0);
    let mut buffer_b8 = temp.remove(0);
    for i in 0..0x22 {
        let x9 = buffer_b0;
        let mut x8 = buffer_b8.rotate_right(8);
        x8 = x8.wrapping_add(x9) ^ i;
        temp.push(x8);
        x8 ^= x9.rotate_right(0x3D);
        hash_table[(i as usize + 1) * 8..(i as usize + 2) * 8].copy_from_slice(&x8.to_le_bytes());
        buffer_b0 = x8;
        buffer_b8 = temp.remove(0);
    }

    let padded = pkcs7_pad(data, 16);
    let mut output = vec![0u8; padded.len()];
    for (index, chunk) in padded.chunks_exact(16).enumerate() {
        output[index * 16..(index + 1) * 16]
            .copy_from_slice(&encrypt_ladon_input(&hash_table, chunk));
    }
    output
}

fn encrypt_ladon_input(hash_table: &[u8], input: &[u8]) -> [u8; 16] {
    let mut data0 = u64_le(&input[..8]);
    let mut data1 = u64_le(&input[8..]);
    for i in 0..0x22 {
        let hash = u64_le(&hash_table[i * 8..(i + 1) * 8]);
        data1 = hash ^ data0.wrapping_add(data1.rotate_right(8));
        data0 = data1 ^ data0.rotate_right(0x3D);
    }
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&data0.to_le_bytes());
    out[8..].copy_from_slice(&data1.to_le_bytes());
    out
}

#[allow(clippy::too_many_arguments)]
fn argus_sign(
    query: &str,
    stub_hex: &str,
    unix: u64,
    aid: u32,
    license_id: u32,
    platform: u32,
    sec_device_id: &str,
    sdk_version: &str,
    sdk_version_int: u32,
    rand: Option<u32>,
) -> String {
    let device_id = query_param(query, "device_id").unwrap_or_default();
    let version_name = query_param(query, "version_name").unwrap_or_default();
    let bean = [
        (1, ProtoValue::Varint((0x2020_0929u64) << 1)),
        (2, ProtoValue::Varint(2)),
        (
            3,
            ProtoValue::Varint(u64::from(rand.unwrap_or_else(random_argus))),
        ),
        (4, ProtoValue::Bytes(aid.to_string().into_bytes())),
        (5, ProtoValue::Bytes(device_id.as_bytes().to_vec())),
        (6, ProtoValue::Bytes(license_id.to_string().into_bytes())),
        (7, ProtoValue::Bytes(version_name.as_bytes().to_vec())),
        (8, ProtoValue::Bytes(sdk_version.as_bytes().to_vec())),
        (9, ProtoValue::Varint(u64::from(sdk_version_int))),
        (10, ProtoValue::Bytes(vec![0; 8])),
        (11, ProtoValue::Varint(u64::from(platform))),
        (12, ProtoValue::Varint(unix << 1)),
        (13, ProtoValue::Bytes(body_hash(stub_hex))),
        (14, ProtoValue::Bytes(query_hash(query))),
        (
            15,
            ProtoValue::Message(vec![
                (1, ProtoValue::Varint(1)),
                (2, ProtoValue::Varint(1)),
                (3, ProtoValue::Varint(1)),
                (7, ProtoValue::Varint(3_348_294_860)),
            ]),
        ),
        (16, ProtoValue::Bytes(sec_device_id.as_bytes().to_vec())),
        (20, ProtoValue::Bytes(b"none".to_vec())),
        (21, ProtoValue::Varint(738)),
        (
            23,
            ProtoValue::Message(vec![
                (1, ProtoValue::Bytes(b"NX551J".to_vec())),
                (2, ProtoValue::Varint(8196)),
                (4, ProtoValue::Varint(2_162_219_008)),
            ]),
        ),
        (25, ProtoValue::Varint(2)),
    ];
    encrypt_argus(&bean)
}

fn body_hash(stub: &str) -> Vec<u8> {
    if stub.is_empty() {
        sm3_hash(&[0; 16])[..6].to_vec()
    } else {
        let decoded = decode_hex(stub).unwrap_or_default();
        sm3_hash(&decoded)[..6].to_vec()
    }
}

fn query_hash(query: &str) -> Vec<u8> {
    if query.is_empty() {
        sm3_hash(&[0; 16])[..6].to_vec()
    } else {
        sm3_hash(query.as_bytes())[..6].to_vec()
    }
}

fn encrypt_argus(bean: &[(u32, ProtoValue)]) -> String {
    let protobuf = pkcs7_pad(&encode_proto(bean), 16);
    let key = &ARGUS_SM3_OUTPUT[..32];
    let mut key_list = [0u64; 4];
    for index in 0..2 {
        let start = index * 16;
        key_list[index * 2] = u64::from_le_bytes(key[start..start + 8].try_into().unwrap());
        key_list[index * 2 + 1] =
            u64::from_le_bytes(key[start + 8..start + 16].try_into().unwrap());
    }

    let mut enc_pb = vec![0u8; protobuf.len()];
    for (index, chunk) in protobuf.chunks_exact(16).enumerate() {
        let pt = [
            u64::from_le_bytes(chunk[..8].try_into().unwrap()),
            u64::from_le_bytes(chunk[8..].try_into().unwrap()),
        ];
        let ct = simon_enc(pt, key_list);
        enc_pb[index * 16..index * 16 + 8].copy_from_slice(&ct[0].to_le_bytes());
        enc_pb[index * 16 + 8..index * 16 + 16].copy_from_slice(&ct[1].to_le_bytes());
    }

    let mut mixed = b"\xf2\xf7\xfc\xff\xf2\xf7\xfc\xff".to_vec();
    mixed.extend_from_slice(&enc_pb);
    let reversed = encrypt_enc_pb(&mixed);
    let mut buffer = b"\xa6n\xad\x9fw\x01\xd0\x0c\x18".to_vec();
    buffer.extend_from_slice(&reversed);
    buffer.extend_from_slice(b"ao");

    let aes_key = md5_digest(&ARGUS_SIGN_KEY[..16]);
    let aes_iv = md5_digest(&ARGUS_SIGN_KEY[16..]);
    let ciphertext = aes128_cbc_encrypt(&aes_key, &aes_iv, &buffer);
    let mut out = b"\xf2\x81".to_vec();
    out.extend_from_slice(&ciphertext);
    base64_encode(&out)
}

fn encrypt_enc_pb(data: &[u8]) -> Vec<u8> {
    let mut bytes = data.to_vec();
    let xor = bytes[..8].to_vec();
    for (index, byte) in bytes.iter_mut().enumerate().skip(8) {
        *byte ^= xor[index % 8];
    }
    bytes.reverse();
    bytes
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then_some(value)
    })
}

fn pkcs7_pad(data: &[u8], block: usize) -> Vec<u8> {
    let pad = block - (data.len() % block);
    let mut out = data.to_vec();
    out.extend(std::iter::repeat_n(pad as u8, pad));
    out
}

fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if !input.len().is_multiple_of(2) {
        return None;
    }
    (0..input.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&input[index..index + 2], 16).ok())
        .collect()
}

fn u64_le(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    buf[..bytes.len().min(8)].copy_from_slice(&bytes[..bytes.len().min(8)]);
    u64::from_le_bytes(buf)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn random_u8() -> u8 {
    uuid::Uuid::new_v4().as_bytes()[0]
}

fn random_4() -> [u8; 4] {
    let bytes = uuid::Uuid::new_v4();
    bytes.as_bytes()[..4].try_into().unwrap()
}

fn random_u64() -> u64 {
    let bytes = uuid::Uuid::new_v4();
    u64::from_le_bytes(bytes.as_bytes()[..8].try_into().unwrap())
}

fn random_argus() -> u32 {
    u32::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap()) & 0x7FFF_FFFF
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUERY: &str = "aweme_id=123&aid=1128&device_id=111&version_name=39.5.0";
    const UNIX: u64 = 1_700_000_000;
    const DEFAULT_LICENSE_ID: u32 = 1_611_921_764;

    #[test]
    fn gorgon_4404_matches_signerpy() {
        let (_, khronos, gorgon) = gorgon(QUERY, "", "", UNIX, 4404);
        assert_eq!(khronos, "1700000000");
        assert_eq!(
            gorgon,
            "0404b0d300004030bd12eb57387ccee15dbc3694a6177ca72dd5"
        );
    }

    #[test]
    fn ladon_matches_signerpy_fixed_random() {
        assert_eq!(
            ladon_encrypt(UNIX, DEFAULT_LICENSE_ID, 1128, Some([1, 2, 3, 4])),
            "AQIDBBRlRCiEas0D6HtJT6OYC08pznxQ7aPFaGChsJ/91J2q"
        );
    }

    #[test]
    fn empty_stub_is_md5_of_empty_payload() {
        let signed = sign_query_at(
            QUERY,
            "",
            UNIX,
            1128,
            DEFAULT_LICENSE_ID,
            4404,
            None,
            Some(42),
        );
        assert_eq!(signed.x_ss_stub, "D41D8CD98F00B204E9800998ECF8427E");
        assert_eq!(
            signed.x_gorgon,
            "0404b0d300004030bd12eb57387ccee15dbc3694a6177ca72dd5"
        );
    }

    #[test]
    fn argus_protobuf_matches_signerpy_fixed_rand() {
        let query = QUERY;
        let stub = md5_hex(b"");
        let encoded = encode_proto(&[
            (1, ProtoValue::Varint((0x2020_0929u64) << 1)),
            (2, ProtoValue::Varint(2)),
            (3, ProtoValue::Varint(42)),
            (4, ProtoValue::Bytes(b"1128".to_vec())),
            (5, ProtoValue::Bytes(b"111".to_vec())),
            (6, ProtoValue::Bytes(b"1611921764".to_vec())),
            (7, ProtoValue::Bytes(b"39.5.0".to_vec())),
            (
                8,
                ProtoValue::Bytes(b"v05.01.02-alpha.7-ov-android".to_vec()),
            ),
            (9, ProtoValue::Varint(83_952_160)),
            (10, ProtoValue::Bytes(vec![0; 8])),
            (11, ProtoValue::Varint(0)),
            (12, ProtoValue::Varint(UNIX << 1)),
            (
                13,
                ProtoValue::Bytes(sm3_hash(&decode_hex(&stub).unwrap())[..6].to_vec()),
            ),
            (
                14,
                ProtoValue::Bytes(sm3_hash(query.as_bytes())[..6].to_vec()),
            ),
            (
                15,
                ProtoValue::Message(vec![
                    (1, ProtoValue::Varint(1)),
                    (2, ProtoValue::Varint(1)),
                    (3, ProtoValue::Varint(1)),
                    (7, ProtoValue::Varint(3_348_294_860)),
                ]),
            ),
            (16, ProtoValue::Bytes(b"SECDEVICETOKEN1".to_vec())),
            (20, ProtoValue::Bytes(b"none".to_vec())),
            (21, ProtoValue::Varint(738)),
            (
                23,
                ProtoValue::Message(vec![
                    (1, ProtoValue::Bytes(b"NX551J".to_vec())),
                    (2, ProtoValue::Varint(8196)),
                    (4, ProtoValue::Varint(2_162_219_008)),
                ]),
            ),
            (25, ProtoValue::Varint(2)),
        ]);
        assert_eq!(
            hex_lower(&encoded),
            "08d2a48082041002182a2204313132382a03313131320a313631313932313736343a0633392e352e30421c7630352e30312e30322d616c7068612e372d6f762d616e64726f696448a08484285208000000000000000058006080c49fd50c6a069c840df1aec0720670f152aa2a187a0c08011001180138ccd9cbbc0c82010f534543444556494345544f4b454e31a201046e6f6e65a801e205ba01110a064e583535314a1084402080b0838708c80102"
        );
    }

    #[test]
    fn argus_matches_signerpy_fixed_rand() {
        let value = argus_sign(
            QUERY,
            &md5_hex(b""),
            UNIX,
            1128,
            DEFAULT_LICENSE_ID,
            0,
            "SECDEVICETOKEN1",
            DEFAULT_SDK_VERSION_STR,
            DEFAULT_SDK_VERSION,
            Some(42),
        );
        assert_eq!(
            value,
            "8oHyrVYEy1xDCDlx64ko40qC6JC9VYY/ZkjJzEl50iSFDw8Yfph1hupLxfKrA9GswSHsfl/Tq+HRReBeJLVFygLW49hrYXjNBZuNiD7ICYc3rXHzsw7kNff+3kzTDafNM+N5dwoyDHfTi9ViW2guJrAW2zlz1pckkG5VDTpqMr8GfDlyANnCP1oM86jqePAIXArm88tcVdR8P2Xw49j5wDOf/L+xwS+HQr0nZs+TBZtifWnlpaak5ZzXFjg37f0HjPNLVsEZhyx7aoJpy/aUmN7DaBv6ts+NKg5/anz4QGfyuQ=="
        );
    }
}
