//! The invariant the bencode module exists to provide.
//!
//! Rewriting resume data means decoding a file, changing a few path fields,
//! and writing it back. Every byte not deliberately changed must survive. The
//! second property below is that guarantee, stated directly.

use std::collections::BTreeMap;

use proptest::prelude::*;
use tsync_core::bencode::{Value, decode, encode};

/// Generates arbitrary bencode values, including nested and non-UTF-8 ones.
fn arb_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(Value::Integer),
        prop::collection::vec(any::<u8>(), 0..48).prop_map(Value::Bytes),
    ];
    leaf.prop_recursive(5, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::List),
            prop::collection::btree_map(prop::collection::vec(any::<u8>(), 0..12), inner, 0..6)
                .prop_map(Value::Dict),
        ]
    })
}

proptest! {
    /// A value survives a trip through the encoder and back.
    #[test]
    fn value_survives_encode_then_decode(value in arb_value()) {
        let bytes = encode(&value);
        let decoded = decode(&bytes).expect("encoder output must be accepted by the decoder");
        prop_assert_eq!(value, decoded);
    }

    /// Bytes survive a trip through the decoder and back, unchanged.
    ///
    /// This is the property that makes rewriting resume data safe.
    #[test]
    fn bytes_survive_decode_then_encode(value in arb_value()) {
        let original = encode(&value);
        let decoded = decode(&original).expect("encoder output must be accepted by the decoder");
        prop_assert_eq!(original, encode(&decoded));
    }
}

/// A structure shaped like real resume data, round-tripped byte for byte.
#[test]
fn realistic_resume_shape_round_trips() {
    let mut inner = BTreeMap::new();
    inner.insert(b"save_path".to_vec(), Value::Bytes(b"/data/music".to_vec()));
    inner.insert(
        b"qBt-savePath".to_vec(),
        Value::Bytes(b"/data/music".to_vec()),
    );
    inner.insert(b"total_uploaded".to_vec(), Value::Integer(123_456_789));
    inner.insert(b"total_downloaded".to_vec(), Value::Integer(0));
    inner.insert(
        b"mapped_files".to_vec(),
        Value::List(vec![
            Value::Bytes(b"album/01 - track.flac".to_vec()),
            Value::Bytes(b"album/02 - track.flac".to_vec()),
        ]),
    );
    // Non-UTF-8 bytes appear in real filenames and must survive untouched.
    inner.insert(
        b"pieces".to_vec(),
        Value::Bytes(vec![0xff, 0x00, 0x80, 0x7f]),
    );

    let value = Value::Dict(inner);
    let encoded = encode(&value);
    let decoded = decode(&encoded).expect("must decode");

    assert_eq!(value, decoded);
    assert_eq!(encoded, encode(&decoded));
}

/// Editing one field changes only that field's bytes.
#[test]
fn rewriting_one_field_leaves_the_rest_untouched() {
    let source = b"d9:save_path4:/old9:untouchedi42ee";
    let mut value = decode(source).expect("fixture must decode");

    value
        .as_dict_mut()
        .expect("fixture is a dictionary")
        .insert(b"save_path".to_vec(), Value::Bytes(b"/new".to_vec()));

    assert_eq!(encode(&value), b"d9:save_path4:/new9:untouchedi42ee");
}
