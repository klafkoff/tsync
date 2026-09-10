//! Torrent metainfo: the `info` dictionary and the files it describes.
//!
//! Pure. The bytes of a `.torrent` file go in; a structured value comes out.
//! Computing the infohash here — SHA-1 of the *encoded* `info` dict — is what
//! lets every other crate name a torrent without touching a disk.

use sha1::{Digest, Sha1};

use crate::bencode::{self, Value};

/// Why a `.torrent` file was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The file is not valid canonical bencode.
    #[error("bencode: {0}")]
    Bencode(#[from] bencode::Error),
    /// The top-level value is not a dictionary.
    #[error("torrent metainfo is not a dictionary")]
    NotADictionary,
    /// The `info` key is missing or not a dictionary.
    #[error("missing or invalid info dictionary")]
    MissingInfo,
    /// `info.name` is missing or not a byte string.
    #[error("info.name is missing or not a byte string")]
    MissingName,
    /// `info.piece length` is missing, not an integer, or not positive.
    #[error("info.piece length is missing or not a positive integer")]
    MissingPieceLength,
    /// Neither `info.length` nor `info.files` is present.
    #[error("info has neither length nor files")]
    MissingFiles,
    /// An entry in `info.files` is malformed.
    #[error("info.files[{index}] is malformed")]
    BadFile {
        /// Position in the `files` list.
        index: usize,
    },
}

/// One content file declared by the torrent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentFile {
    /// Path components relative to the torrent name (multi-file) or unused
    /// (single-file).
    pub path: Vec<Vec<u8>>,
    /// Declared length in bytes.
    pub length: u64,
}

/// Decoded torrent metainfo that audit and rewrite both need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Metainfo {
    /// SHA-1 of the canonical encoding of `info`.
    pub infohash: [u8; 20],
    /// `info.name` — a directory (multi-file) or the filename (single-file).
    pub name: Vec<u8>,
    /// Piece length in bytes.
    pub piece_length: u64,
    /// Declared files, in torrent order.
    pub files: Vec<ContentFile>,
    /// Whether `info.files` was present (multi-file layout).
    pub multi_file: bool,
}

impl Metainfo {
    /// 40-character lowercase hex infohash.
    #[must_use]
    pub fn infohash_hex(&self) -> String {
        hex_lower(&self.infohash)
    }

    /// Total declared size of every file.
    #[must_use]
    pub fn total_length(&self) -> u64 {
        self.files.iter().map(|file| file.length).sum()
    }

    /// Number of pieces implied by the total length and piece length.
    #[must_use]
    pub fn piece_count(&self) -> usize {
        let total = self.total_length();
        if total == 0 {
            return 1;
        }
        let piece = self.piece_length.max(1);
        usize::try_from(total.div_ceil(piece)).unwrap_or(usize::MAX)
    }
}

/// Decodes a `.torrent` file.
///
/// Announce URLs and other top-level keys are ignored. Callers that must not
/// leak a passkey should never print the raw input, only this struct.
///
/// # Errors
///
/// Returns [`Error`] when the input is not canonical bencode or the `info`
/// dictionary is missing required fields.
pub fn parse(bytes: &[u8]) -> Result<Metainfo, Error> {
    let top = bencode::decode(bytes)?;
    let dict = top.as_dict().ok_or(Error::NotADictionary)?;
    let info = dict.get(b"info".as_slice()).ok_or(Error::MissingInfo)?;
    if info.as_dict().is_none() {
        return Err(Error::MissingInfo);
    }

    let infohash = sha1_20(&bencode::encode(info));
    let name = info
        .get(b"name")
        .and_then(Value::as_bytes)
        .ok_or(Error::MissingName)?
        .to_vec();

    let piece_length = info
        .get(b"piece length")
        .and_then(Value::as_integer)
        .filter(|n| *n > 0)
        .and_then(|n| u64::try_from(n).ok())
        .ok_or(Error::MissingPieceLength)?;

    let (files, multi_file) = files_from_info(info)?;

    Ok(Metainfo {
        infohash,
        name,
        piece_length,
        files,
        multi_file,
    })
}

fn files_from_info(info: &Value) -> Result<(Vec<ContentFile>, bool), Error> {
    if let Some(length) = info.get(b"length").and_then(Value::as_integer) {
        let length = u64::try_from(length).map_err(|_| Error::MissingFiles)?;
        return Ok((
            vec![ContentFile {
                path: Vec::new(),
                length,
            }],
            false,
        ));
    }

    let Some(list) = info.get(b"files").and_then(Value::as_list) else {
        return Err(Error::MissingFiles);
    };

    let mut files = Vec::with_capacity(list.len());
    for (index, entry) in list.iter().enumerate() {
        let length = entry
            .get(b"length")
            .and_then(Value::as_integer)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(Error::BadFile { index })?;
        let path = entry
            .get(b"path")
            .and_then(Value::as_list)
            .ok_or(Error::BadFile { index })?;
        let components: Option<Vec<Vec<u8>>> = path
            .iter()
            .map(|component| component.as_bytes().map(ToOwned::to_owned))
            .collect();
        let path = components.ok_or(Error::BadFile { index })?;
        if path.is_empty() {
            return Err(Error::BadFile { index });
        }
        files.push(ContentFile { path, length });
    }

    Ok((files, true))
}

fn sha1_20(bytes: &[u8]) -> [u8; 20] {
    let digest = Sha1::digest(bytes);
    let mut out = [0_u8; 20];
    out.copy_from_slice(&digest);
    out
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}
