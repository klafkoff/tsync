//! qBittorrent / libtorrent `.fastresume` fields that audit needs.
//!
//! Only the keys that decide *where the files are* and *what the client
//! believes* are parsed. Everything else is ignored so a future libtorrent
//! field cannot break this module.

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

/// Piece 0 is the high bit of byte 0, matching libtorrent's bitfield layout.
fn bit_is_set(field: &[u8], index: usize) -> bool {
    let byte = index / 8;
    let shift = 7 - (index % 8);
    field
        .get(byte)
        .is_some_and(|value| value & (1 << shift) != 0)
}
