//! VLQ (Variable Length Quantity) decoder for source maps
//!
//! This module implements Base64 VLQ decoding used in source map v3 format.
//! VLQ encoding is used in the `mappings` field to efficiently store position data.

/// Base64 VLQ alphabet used in source maps
const VLQ_BASE64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Decode a single Base64 character to its numeric value (0-63)
fn decode_base64_char(c: char) -> Option<i32> {
    VLQ_BASE64_CHARS
        .iter()
        .position(|&ch| ch == c as u8)
        .map(|pos| pos as i32)
}

/// Decode a single VLQ value from the character iterator
///
/// VLQ encoding:
/// - Each character encodes 6 bits
/// - Bit 5 (0x20) is the continuation bit: 1 = more chars follow, 0 = last char
/// - Bit 0 of the FIRST char is the sign bit
/// - Remaining 4 bits (first char) or 5 bits (subsequent chars) are value bits
///
/// Example: "C" = 2 (binary: 000010, no continuation, value = 1, positive)
///          "D" = 3 (binary: 000011, no continuation, value = 1, negative = -1)
pub fn decode_vlq_value(chars: &mut impl Iterator<Item = char>) -> Option<i32> {
    let mut result;
    let mut shift;
    let mut continuation;

    // Read the first character to get the sign bit
    let first_char = chars.next()?;
    let mut value = decode_base64_char(first_char)?;

    // Bit 0 is the sign bit (only in first char)
    let negative = (value & 1) != 0;

    // Bits 1-4 are value bits in first char
    result = (value >> 1) & 0xF;
    shift = 4;

    // Bit 5 is continuation bit
    continuation = (value & 0x20) != 0;

    // Read remaining characters if continuation bit is set
    while continuation {
        let ch = chars.next()?;
        value = decode_base64_char(ch)?;

        // Bits 0-4 are value bits in subsequent chars
        result |= (value & 0x1F) << shift;
        shift += 5;

        // Bit 5 is continuation bit
        continuation = (value & 0x20) != 0;
    }

    // Apply sign
    Some(if negative { -result } else { result })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_vlq_single_char() {
        // "A" = 0 (binary: 000000)
        let mut chars = "A".chars();
        assert_eq!(decode_vlq_value(&mut chars), Some(0));

        // "C" = 1 (binary: 000010, bit 0=0 (positive), bits 1-4=1)
        let mut chars = "C".chars();
        assert_eq!(decode_vlq_value(&mut chars), Some(1));

        // "D" = -1 (binary: 000011, bit 0=1 (negative), bits 1-4=1)
        let mut chars = "D".chars();
        assert_eq!(decode_vlq_value(&mut chars), Some(-1));
    }

    #[test]
    fn test_decode_vlq_multi_char() {
        // "gC" = 32 (multi-byte VLQ)
        let mut chars = "gC".chars();
        assert_eq!(decode_vlq_value(&mut chars), Some(32));

        // "hC" = -32 (multi-byte VLQ, negative)
        let mut chars = "hC".chars();
        assert_eq!(decode_vlq_value(&mut chars), Some(-32));
    }

    #[test]
    fn test_decode_base64_char() {
        assert_eq!(decode_base64_char('A'), Some(0));
        assert_eq!(decode_base64_char('Z'), Some(25));
        assert_eq!(decode_base64_char('a'), Some(26));
        assert_eq!(decode_base64_char('z'), Some(51));
        assert_eq!(decode_base64_char('0'), Some(52));
        assert_eq!(decode_base64_char('9'), Some(61));
        assert_eq!(decode_base64_char('+'), Some(62));
        assert_eq!(decode_base64_char('/'), Some(63));
        assert_eq!(decode_base64_char('!'), None); // Invalid char
    }
}

#[cfg(test)]
mod vlq_tests {
    use super::*;

    #[test]
    fn test_decode_base64_char_basic() {
        assert_eq!(decode_base64_char('A'), Some(0));
        assert_eq!(decode_base64_char('Z'), Some(25));
        assert_eq!(decode_base64_char('a'), Some(26));
        assert_eq!(decode_base64_char('z'), Some(51));
        assert_eq!(decode_base64_char('0'), Some(52));
        assert_eq!(decode_base64_char('9'), Some(61));
        assert_eq!(decode_base64_char('+'), Some(62));
        assert_eq!(decode_base64_char('/'), Some(63));
        assert_eq!(decode_base64_char('!'), None); // Invalid char
    }

    #[test]
    fn test_decode_vlq_single_char_values() {
        // "A" = 0 (binary: 000000)
        let mut chars = "A".chars();
        assert_eq!(decode_vlq_value(&mut chars), Some(0));

        // "C" = 1 (binary: 000010, bit 0=0 (positive), bits 1-4=1)
        let mut chars = "C".chars();
        assert_eq!(decode_vlq_value(&mut chars), Some(1));

        // "D" = -1 (binary: 000011, bit 0=1 (negative), bits 1-4=1)
        let mut chars = "D".chars();
        assert_eq!(decode_vlq_value(&mut chars), Some(-1));

        // "E" = 2
        let mut chars = "E".chars();
        assert_eq!(decode_vlq_value(&mut chars), Some(2));

        // "F" = -2
        let mut chars = "F".chars();
        assert_eq!(decode_vlq_value(&mut chars), Some(-2));
    }

    #[test]
    fn test_decode_vlq_multi_char_values() {
        // "gC" = 32 (multi-byte VLQ)
        let mut chars = "gC".chars();
        assert_eq!(decode_vlq_value(&mut chars), Some(32));

        // "hC" = -32 (multi-byte VLQ, negative)
        let mut chars = "hC".chars();
        assert_eq!(decode_vlq_value(&mut chars), Some(-32));
    }
}
