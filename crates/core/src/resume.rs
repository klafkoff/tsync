//! qBittorrent / libtorrent `.fastresume` fields that audit and rewrite need.
//!
//! Path rewriting is the load-bearing operation: decode, prove a byte-identical
//! round trip, replace only the path keys that already exist, encode again.

use crate::bencode::{self, Value};

/// Why a `.fastresume` file was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The file is not valid canonical bencode.
    #[error("bencode: {0}")]
    Bencode(#[from] bencode::Error),
    /// The top-level value is not a dictionary.
    #[error("fastresume is not a dictionary")]
    NotADictionary,
    /// No usable save path (`save_path` or `qBt-savePath`).
    #[error("fastresume has no save_path")]
    MissingSavePath,
    /// `encode(decode(bytes))` did not reproduce the input. Refusing to write
    /// prevents a silent rewrite of fields we did not intend to change.
    #[error("fastresume does not round-trip through canonical bencode")]
    RoundTripMismatch,
}

/// Resume fields that locate and classify a torrent's files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resume {
    /// Directory the content lives under. Multi-file torrents put `info.name`
    /// inside this; single-file torrents put the file here.
    pub save_path: Vec<u8>,
    /// Per-file priority, if present. `0` means the user deselected the file.
    pub file_priority: Option<Vec<i64>>,
    /// libtorrent piece bitfield, if present. All-ones means the client
    /// claims every piece is present.
    pub pieces: Option<Vec<u8>>,
}

impl Resume {
    /// Whether the bitfield claims every piece is present.
    ///
    /// `None` if there is no bitfield, so we cannot know what the client
    /// thinks. A missing bitfield is not treated as a claim.
    #[must_use]
    pub fn claims_complete(&self, piece_count: usize) -> Option<bool> {
        let field = self.pieces.as_deref()?;
        if piece_count == 0 {
            return Some(true);
        }
        Some((0..piece_count).all(|index| bit_is_set(field, index)))
    }

    /// Priority for file `index`, or `1` (normal) when the list is absent.
    #[must_use]
    pub fn priority(&self, index: usize) -> i64 {
        self.file_priority
            .as_ref()
            .and_then(|list| list.get(index).copied())
            .unwrap_or(1)
    }
}

/// Decodes a `.fastresume` file.
///
/// # Errors
///
/// Returns [`Error`] when the input is not canonical bencode or has no save
/// path.
pub fn parse(bytes: &[u8]) -> Result<Resume, Error> {
    let top = bencode::decode(bytes)?;
    let save_path = top
        .get(b"save_path")
        .or_else(|| top.get(b"qBt-savePath"))
        .and_then(Value::as_bytes)
        .ok_or(Error::MissingSavePath)?
        .to_vec();

    let file_priority = top
        .get(b"file_priority")
        .and_then(Value::as_list)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_integer)
                .collect::<Vec<i64>>()
        });

    let pieces = top
        .get(b"pieces")
        .and_then(Value::as_bytes)
        .map(ToOwned::to_owned);

    Ok(Resume {
        save_path,
        file_priority,
        pieces,
    })
}

/// Path keys rewritten in a `.fastresume`. Only keys that already exist are
/// replaced — we never invent qBittorrent fields the client did not write.
const PATH_KEYS: [&[u8]; 3] = [b"save_path", b"qBt-savePath", b"qBt-downloadPath"];

/// Rewrites save-path fields after proving a byte-identical round trip.
///
/// If `encode(decode(bytes))` is not `bytes`, the file is refused rather than
/// normalized. That is what keeps an encoder disagreement from silently
/// rewriting fields nobody asked to change.
///
/// # Errors
///
/// Returns [`Error`] when the input is not canonical, is not a dictionary, or
/// has no save path to replace.
pub fn rewrite_save_path(bytes: &[u8], new_save: &[u8]) -> Result<Vec<u8>, Error> {
    let mut value = bencode::decode(bytes)?;
    if bencode::encode(&value) != bytes {
        return Err(Error::RoundTripMismatch);
    }

    let dict = value.as_dict_mut().ok_or(Error::NotADictionary)?;
    let mut replaced = false;
    for key in PATH_KEYS {
        if dict.contains_key(key) {
            dict.insert(key.to_vec(), Value::Bytes(new_save.to_vec()));
            replaced = true;
        }
    }
    if !replaced {
        return Err(Error::MissingSavePath);
    }

    Ok(bencode::encode(&value))
}

/// Piece 0 is the high bit of byte 0, matching libtorrent's bitfield layout.
fn bit_is_set(field: &[u8], index: usize) -> bool {
    let byte = index / 8;
    let shift = 7 - (index % 8);
    field
        .get(byte)
        .is_some_and(|value| value & (1 << shift) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bencode::{self, Value};
    use std::collections::BTreeMap;

    fn resume_bytes() -> Vec<u8> {
        let mut map = BTreeMap::new();
        map.insert(b"save_path".to_vec(), Value::Bytes(b"/old".to_vec()));
        map.insert(b"qBt-savePath".to_vec(), Value::Bytes(b"/old".to_vec()));
        map.insert(b"paused".to_vec(), Value::Integer(1));
        bencode::encode(&Value::Dict(map))
    }

    #[test]
    fn rewrite_changes_only_the_path_keys() {
        let original = resume_bytes();
        let rewritten = rewrite_save_path(&original, b"/new").expect("rewrite");

        let old = bencode::decode(&original).expect("decode old");
        let new = bencode::decode(&rewritten).expect("decode new");
        assert_eq!(
            new.get(b"save_path").and_then(Value::as_bytes),
            Some(b"/new".as_slice())
        );
        assert_eq!(new.get(b"paused").and_then(Value::as_integer), Some(1));
        assert_ne!(rewritten, original);
        assert_eq!(old.get(b"paused"), new.get(b"paused"));
    }

    #[test]
    fn rewrite_refuses_a_value_that_is_not_a_dictionary() {
        assert!(rewrite_save_path(b"i1e", b"/new").is_err());
    }
}
