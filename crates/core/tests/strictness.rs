//! The decoder accepts canonical bencode and nothing else.
//!
//! Every rejection here is a byte sequence that some other parser would accept
//! and silently normalize. Normalizing is exactly what must not happen when the
//! output is written back over a torrent's resume data, so each of these is a
//! guard against silent corruption rather than mere pedantry.

use tsync_core::bencode::{decode, encode};

/// Canonical inputs, which must decode and re-encode to the identical bytes.
const ACCEPTED: &[(&str, &[u8])] = &[
    ("zero", b"i0e"),
    ("negative integer", b"i-42e"),
    ("i64::MAX", b"i9223372036854775807e"),
    ("i64::MIN", b"i-9223372036854775808e"),
    ("empty byte string", b"0:"),
    ("byte string", b"4:spam"),
    ("empty list", b"le"),
    ("empty dictionary", b"de"),
    ("sorted dictionary", b"d1:ai1e1:bi2ee"),
    ("nested structures", b"d1:ald1:bi1eeee"),
    ("list of strings", b"l4:spam4:eggse"),
];

/// Non-canonical or malformed inputs, which must be rejected.
const REJECTED: &[(&str, &[u8])] = &[
    ("empty input", b""),
    ("bare terminator", b"e"),
    ("integer with leading zero", b"i03e"),
    ("negative zero", b"i-0e"),
    ("empty integer", b"ie"),
    ("bare minus", b"i-e"),
    ("non-digit in integer", b"i1a2e"),
    ("unterminated integer", b"i42"),
    ("i64 overflow", b"i9223372036854775808e"),
    ("length with leading zero", b"03:abc"),
    ("truncated byte string", b"5:abc"),
    ("missing length", b":abc"),
    ("unterminated list", b"li1e"),
    ("unterminated dictionary", b"d1:ai1e"),
    ("unsorted dictionary keys", b"d1:bi1e1:ai2ee"),
    ("duplicate dictionary keys", b"d1:ai1e1:ai2ee"),
    ("integer as dictionary key", b"di1ei2ee"),
    ("list as dictionary key", b"dlei2ee"),
    ("dictionary missing value", b"d1:ae"),
    ("trailing data", b"i1ee"),
    ("trailing whitespace", b"i1e\n"),
    ("two values", b"i1ei2e"),
];

#[test]
fn accepts_canonical_input() {
    for (name, input) in ACCEPTED {
        let value = decode(input).unwrap_or_else(|error| {
            panic!("{name}: expected {input:?} to decode, got error: {error}")
        });
        assert_eq!(
            &encode(&value),
            input,
            "{name}: re-encoding must reproduce the input byte for byte"
        );
    }
}

#[test]
fn rejects_non_canonical_input() {
    for (name, input) in REJECTED {
        assert!(
            decode(input).is_err(),
            "{name}: expected {input:?} to be rejected, but it decoded"
        );
    }
}

/// Deep nesting is refused rather than exhausting the stack.
#[test]
fn rejects_input_nested_past_the_depth_limit() {
    let depth = tsync_core::bencode::MAX_DEPTH + 10;
    let mut input = vec![b'l'; depth];
    input.extend(std::iter::repeat_n(b'e', depth));

    assert!(
        decode(&input).is_err(),
        "nesting past MAX_DEPTH must be rejected"
    );
}

/// Nesting just inside the limit is still accepted.
#[test]
fn accepts_input_nested_within_the_depth_limit() {
    let depth = tsync_core::bencode::MAX_DEPTH - 1;
    let mut input = vec![b'l'; depth];
    input.extend(std::iter::repeat_n(b'e', depth));

    assert!(
        decode(&input).is_ok(),
        "nesting within MAX_DEPTH must decode"
    );
}
