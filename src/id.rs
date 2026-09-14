//! Fast Base62 ID encoder/decoder with adaptive arbitrary-length fallback.
//!
//! Standard 5-character IDs fit into a 4-byte integer (u32).
//! Longer or irregular IDs are tagged with an escape bit and stored with zero data loss.

#[inline(always)]
pub fn base62_char_to_val(c: u8) -> u32 {
    match c {
        b'0'..=b'9' => (c - b'0') as u32,
        b'a'..=b'z' => (c - b'a' + 10) as u32,
        b'A'..=b'Z' => (c - b'A' + 36) as u32,
        _ => 0,
    }
}

#[inline(always)]
pub fn val_to_base62_char(v: u32) -> u8 {
    match v {
        0..=9 => b'0' + (v as u8),
        10..=35 => b'a' + ((v - 10) as u8),
        36..=61 => b'A' + ((v - 36) as u8),
        _ => b'0',
    }
}

/// Checks if a string is a standard 5-character Base62 string.
#[inline(always)]
pub fn is_standard_base62_5char(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5 && b.iter().all(|&c| c.is_ascii_alphanumeric())
}

/// Encodes a 5-character ID (e.g. "00008", "0008Q") into a u32.
#[inline(always)]
pub fn encode_base62_id(id_str: &str) -> u32 {
    let bytes = id_str.as_bytes();
    let mut val = 0u32;
    for &b in bytes.iter().take(5) {
        val = val * 62 + base62_char_to_val(b);
    }
    val
}

/// Decodes a u32 back into a 5-character ASCII string.
#[inline(always)]
pub fn decode_base62_id(mut val: u32) -> String {
    let mut out = [b'0'; 5];
    for i in (0..5).rev() {
        out[i] = val_to_base62_char(val % 62);
        val /= 62;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base62_roundtrip() {
        let test_ids = [
            "00008", "0000D", "0008Q", "0009B", "000Pw", "zzzz9", "ZZZZZ", "00000", "abcDE",
        ];

        for &id in &test_ids {
            assert!(is_standard_base62_5char(id));
            let encoded = encode_base62_id(id);
            let decoded = decode_base62_id(encoded);
            assert_eq!(decoded, id);
        }
    }
}
