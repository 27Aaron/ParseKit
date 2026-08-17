//! ByteDance mobile request signatures used by Douyin.

mod crypto;
mod headers;

pub use headers::{SignedHeaders, sign_query, trace_id};

/// Percent-encodes like Python `urllib.parse.quote_plus` with empty `safe`.
pub fn quote_plus(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(char::from(hex_digit(byte >> 4)));
                out.push(char::from(hex_digit(byte & 0x0f)));
            }
        }
    }
    out
}

pub fn encode_query(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", quote_plus(key), quote_plus(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn hex_digit(value: u8) -> u8 {
    b"0123456789ABCDEF"[value as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_plus_matches_python_urlencode() {
        assert_eq!(quote_plus("1080*2400"), "1080%2A2400");
        assert_eq!(quote_plus("Asia/Shanghai"), "Asia%2FShanghai");
        assert_eq!(quote_plus("39.5.0"), "39.5.0");
    }
}
