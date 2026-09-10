//! Synthetic torrent libraries for tests.
//!
//! Generated, never captured. A test describes the library it needs — how many
//! torrents, which files are missing, which save paths — and this crate writes
//! a `BT_backup` plus matching data files. The same description produces the
//! same bytes on every machine, so the suite never reads a real library and
//! cannot leak one.
//!
//! Announce URLs use the `.invalid` TLD (RFC 2606). Save paths live under
//! `/srv`, never under a real home directory.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};
use tsync_core::bencode::{self, Value};

/// Something went wrong while writing a fixture to disk.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A filesystem operation failed.
    #[error("writing fixture: {0}")]
    Io(#[from] io::Error),
}

/// How the on-disk files relate to what the resume data claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// Every file is on disk at its full length.
    Complete,
    /// The first `count` files are absent; resume data still claims complete.
    ///
    /// This is the disagreement that motivates the import guards: a client
    /// that trusts resume data would seed (or skip a recheck) while the
    /// content is gone.
    ClaimsCompleteButMissing {
        /// How many leading files to omit.
        count: usize,
    },
}

/// One file inside a synthetic torrent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSpec {
    /// Path relative to the torrent name, using `/` separators.
    pub relative: String,
    /// Length in bytes.
    pub length: u64,
}

/// A torrent to materialize.
#[derive(Clone, Debug)]
pub struct TorrentSpec {
    /// Display name, also the directory under `save_path`.
    pub name: String,
    /// Absolute save path, as resume data will record it.
    pub save_path: String,
    /// Files in this torrent, in order.
    pub files: Vec<FileSpec>,
    /// What is actually written under the data root.
    pub presence: Presence,
}

/// One materialized torrent.
#[derive(Clone, Debug)]
pub struct Torrent {
    /// 40-character lowercase hex infohash, and the stem of both backup files.
    pub infohash: String,
    /// The spec this was built from.
    pub spec: TorrentSpec,
    /// `<bt_backup>/<infohash>.torrent`
    pub torrent_path: PathBuf,
    /// `<bt_backup>/<infohash>.fastresume`
    pub resume_path: PathBuf,
}

/// A generated library on disk.
#[derive(Debug)]
pub struct Library {
    /// Directory holding paired `.torrent` / `.fastresume` files.
    pub bt_backup: PathBuf,
    /// Directory holding content files, laid out as `save_path` relative to
    /// a synthetic root so tests never write to `/srv`.
    pub data_root: PathBuf,
    /// Torrents, in the order they were added to the builder.
    pub torrents: Vec<Torrent>,
}

/// Accumulates torrents and writes them in one pass.
#[derive(Default)]
pub struct Builder {
    torrents: Vec<TorrentSpec>,
}

impl Builder {
    /// An empty library.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a fully-present torrent.
    #[must_use]
    pub fn complete(
        mut self,
        name: impl Into<String>,
        save_path: impl Into<String>,
        files: &[(&str, u64)],
    ) -> Self {
        self.torrents.push(TorrentSpec {
            name: name.into(),
            save_path: save_path.into(),
            files: file_specs(files),
            presence: Presence::Complete,
        });
        self
    }

    /// Adds a torrent whose resume data claims complete and whose first
    /// `missing` files are not on disk.
    #[must_use]
    pub fn claims_complete_but_missing(
        mut self,
        name: impl Into<String>,
        save_path: impl Into<String>,
        files: &[(&str, u64)],
        missing: usize,
    ) -> Self {
        self.torrents.push(TorrentSpec {
            name: name.into(),
            save_path: save_path.into(),
            files: file_specs(files),
            presence: Presence::ClaimsCompleteButMissing { count: missing },
        });
        self
    }

    /// Writes `BT_backup` and content under `root`.
    ///
    /// `root/BT_backup` gets the paired metadata. `root/data` gets file
    /// contents. Resume `save_path` values stay as specified (e.g. `/srv/music`)
    /// so rewrite tests have a realistic prefix to replace; only the bytes on
    /// disk are relocated under `root/data`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if any directory or file cannot be created.
    pub fn materialize(self, root: &Path) -> Result<Library, Error> {
        let bt_backup = root.join("BT_backup");
        let data_root = root.join("data");
        fs::create_dir_all(&bt_backup)?;
        fs::create_dir_all(&data_root)?;

        let mut torrents = Vec::with_capacity(self.torrents.len());
        for (index, spec) in self.torrents.into_iter().enumerate() {
            torrents.push(write_torrent(&bt_backup, &data_root, index, spec)?);
        }

        Ok(Library {
            bt_backup,
            data_root,
            torrents,
        })
    }
}

fn file_specs(files: &[(&str, u64)]) -> Vec<FileSpec> {
    files
        .iter()
        .map(|(relative, length)| FileSpec {
            relative: (*relative).to_owned(),
            length: *length,
        })
        .collect()
}

fn write_torrent(
    bt_backup: &Path,
    data_root: &Path,
    index: usize,
    spec: TorrentSpec,
) -> Result<Torrent, Error> {
    let payload = concatenated_payload(index, &spec.files);
    let info = info_dict(&spec, &payload);
    let infohash = infohash_hex(&info);

    let metainfo = dict([
        (
            b"announce".as_slice(),
            bytes(b"http://tracker.invalid/announce"),
        ),
        (b"info", info),
    ]);
    let resume = resume_dict(&spec);

    let torrent_path = bt_backup.join(format!("{infohash}.torrent"));
    let resume_path = bt_backup.join(format!("{infohash}.fastresume"));
    fs::write(&torrent_path, bencode::encode(&metainfo))?;
    fs::write(&resume_path, bencode::encode(&resume))?;

    write_payload(data_root, &spec, index)?;

    Ok(Torrent {
        infohash,
        spec,
        torrent_path,
        resume_path,
    })
}

/// Piece length is small so fixtures stay tiny while still spanning pieces.
const PIECE_LENGTH: i64 = 16_384;

fn info_dict(spec: &TorrentSpec, payload: &[u8]) -> Value {
    let files = spec
        .files
        .iter()
        .map(|file| {
            dict([
                (
                    b"length".as_slice(),
                    Value::Integer(
                        i64::try_from(file.length).expect("fixture file length fits i64"),
                    ),
                ),
                (b"path", path_list(&file.relative)),
            ])
        })
        .collect();

    dict([
        (b"files".as_slice(), Value::List(files)),
        (b"name", bytes(spec.name.as_bytes())),
        (b"piece length", Value::Integer(PIECE_LENGTH)),
        (b"pieces", Value::Bytes(piece_hashes(payload))),
        (b"private", Value::Integer(1)),
        (b"source", bytes(b"synthetic")),
    ])
}

fn resume_dict(spec: &TorrentSpec) -> Value {
    dict([
        (b"paused".as_slice(), Value::Integer(1)),
        (b"qBt-downloadPath", bytes(spec.save_path.as_bytes())),
        (b"qBt-savePath", bytes(spec.save_path.as_bytes())),
        (b"save_path", bytes(spec.save_path.as_bytes())),
        (b"total_downloaded", Value::Integer(0)),
        (b"total_uploaded", Value::Integer(0)),
    ])
}

fn path_list(relative: &str) -> Value {
    Value::List(
        relative
            .split('/')
            .map(|component| bytes(component.as_bytes()))
            .collect(),
    )
}

fn concatenated_payload(index: usize, files: &[FileSpec]) -> Vec<u8> {
    let mut out = Vec::new();
    for (file_ix, file) in files.iter().enumerate() {
        out.extend(file_bytes(
            index,
            file_ix,
            usize::try_from(file.length).expect("fixture files fit in memory"),
        ));
    }
    out
}

fn file_bytes(torrent_ix: usize, file_ix: usize, len: usize) -> Vec<u8> {
    let torrent_ix = u64::try_from(torrent_ix).expect("torrent index fits u64");
    let file_ix = u64::try_from(file_ix).expect("file index fits u64");
    (0..len)
        .map(|offset| {
            let offset = u64::try_from(offset).expect("offset fits u64");
            let byte = torrent_ix
                .wrapping_mul(31)
                .wrapping_add(file_ix.wrapping_mul(17))
                .wrapping_add(offset)
                .wrapping_rem(251);
            u8::try_from(byte).expect("251 fits u8")
        })
        .collect()
}

fn piece_hashes(payload: &[u8]) -> Vec<u8> {
    if payload.is_empty() {
        return Sha1::digest([]).to_vec();
    }
    payload
        .chunks(usize::try_from(PIECE_LENGTH).expect("piece length fits usize"))
        .flat_map(|piece| Sha1::digest(piece).to_vec())
        .collect()
}

fn write_payload(data_root: &Path, spec: &TorrentSpec, index: usize) -> Result<(), Error> {
    let skip = match spec.presence {
        Presence::Complete => 0,
        Presence::ClaimsCompleteButMissing { count } => count,
    };

    for (file_ix, file) in spec.files.iter().enumerate() {
        if file_ix < skip {
            continue;
        }
        let dest = data_root
            .join(spec.save_path.trim_start_matches('/'))
            .join(&spec.name)
            .join(&file.relative);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = file_bytes(
            index,
            file_ix,
            usize::try_from(file.length).expect("fixture files fit in memory"),
        );
        fs::write(dest, bytes)?;
    }
    Ok(())
}

fn infohash_hex(info: &Value) -> String {
    let digest = Sha1::digest(bencode::encode(info));
    hex_lower(&digest)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn bytes(raw: &[u8]) -> Value {
    Value::Bytes(raw.to_vec())
}

fn dict<const N: usize>(pairs: [(&[u8], Value); N]) -> Value {
    let mut map = BTreeMap::new();
    for (key, value) in pairs {
        map.insert(key.to_vec(), value);
    }
    Value::Dict(map)
}
