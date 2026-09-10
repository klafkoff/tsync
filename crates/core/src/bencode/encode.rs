//! Canonical bencode encoding.
//!
//! The encoder has no configuration and no choices to make. Dictionary order
//! comes from [`BTreeMap`](std::collections::BTreeMap), integers have exactly
//! one spelling, and lengths are written without padding. There is precisely
//! one byte sequence for any given [`Value`], which is what allows the decoder
//! to be strict about accepting only that sequence.

use super::Value;

/// Encodes `value` as canonical bencode.
#[must_use]
pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(value, &mut out);
    out
}

/// Encodes `value` as canonical bencode, appending to `out`.
///
/// Useful when writing several values into one buffer, or when reusing an
/// allocation across many encodes.
pub fn encode_into(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Integer(n) => {
            out.push(b'i');
            out.extend_from_slice(n.to_string().as_bytes());
            out.push(b'e');
        }
        Value::Bytes(bytes) => push_byte_string(bytes, out),
        Value::List(items) => {
            out.push(b'l');
            for item in items {
                encode_into(item, out);
            }
            out.push(b'e');
        }
        Value::Dict(map) => {
            out.push(b'd');
            for (key, item) in map {
                push_byte_string(key, out);
                encode_into(item, out);
            }
            out.push(b'e');
        }
    }
}

fn push_byte_string(bytes: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(bytes.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(bytes);
}
