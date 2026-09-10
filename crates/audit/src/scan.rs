//! Walk `BT_backup`, pair files, stat content, build a manifest.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tsync_core::{metainfo, resume};

use crate::classify::{self, FileView, Observed};
use crate::manifest::{Entry, Manifest};
use crate::paths::{content_path, relative_name};

/// How to find the library.
#[derive(Clone, Debug)]
pub struct Options {
    /// qBittorrent `BT_backup` directory.
    pub bt_backup: PathBuf,
    /// When set, save paths are resolved relative to this directory.
    pub data_root: Option<PathBuf>,
}

/// Why the scan could not produce a manifest.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `BT_backup` could not be listed.
    #[error("cannot list {path}: {source}")]
    List {
        /// Directory that failed.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
}

impl Error {
    /// Whether this looks like a macOS TCC denial rather than a missing dir.
    #[must_use]
    pub fn is_permission_denied(&self) -> bool {
        matches!(self, Self::List { source, .. } if source.kind() == io::ErrorKind::PermissionDenied)
    }
}

/// Scans `opts.bt_backup` and returns a manifest.
///
/// # Errors
///
/// Returns [`Error::List`] when the backup directory cannot be read. Individual
/// torrents that fail to parse become entries with an empty name rather than
/// aborting the scan.
pub fn run(opts: &Options) -> Result<Manifest, Error> {
    let listed = fs::read_dir(&opts.bt_backup).map_err(|source| Error::List {
        path: opts.bt_backup.clone(),
        source,
    })?;

    let mut pairs: BTreeMap<String, Pair> = BTreeMap::new();
    for entry in listed.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        let slot = pairs.entry(stem.to_owned()).or_default();
        match ext {
            "torrent" => slot.torrent = Some(path),
            "fastresume" => slot.resume = Some(path),
            _ => {}
        }
    }

    let mut torrents = Vec::new();
    let mut unpaired_torrent = 0;
    let mut unpaired_resume = 0;

    for (stem, pair) in pairs {
        match (pair.torrent, pair.resume) {
            (Some(torrent), Some(resume_path)) => {
                torrents.push(inspect_pair(
                    stem,
                    &torrent,
                    &resume_path,
                    opts.data_root.as_deref(),
                ));
            }
            (Some(_), None) => unpaired_torrent += 1,
            (None, Some(_)) => unpaired_resume += 1,
            (None, None) => {}
        }
    }

    Ok(Manifest {
        bt_backup: opts.bt_backup.display().to_string(),
        torrents,
        unpaired_torrent,
        unpaired_resume,
    })
}

#[derive(Default)]
struct Pair {
    torrent: Option<PathBuf>,
    resume: Option<PathBuf>,
}

fn inspect_pair(
    stem: String,
    torrent_path: &Path,
    resume_path: &Path,
    data_root: Option<&Path>,
) -> Entry {
    let Ok(torrent_bytes) = fs::read(torrent_path) else {
        return unverifiable(stem, String::new(), String::new(), 0);
    };
    let Ok(resume_bytes) = fs::read(resume_path) else {
        return unverifiable(stem, String::new(), String::new(), 0);
    };

    let Ok(meta) = metainfo::parse(&torrent_bytes) else {
        return unverifiable(stem, String::new(), String::new(), 0);
    };
    let Ok(resume) = resume::parse(&resume_bytes) else {
        return unverifiable(
            meta.infohash_hex(),
            String::from_utf8_lossy(&meta.name).into_owned(),
            String::new(),
            meta.total_length(),
        );
    };

    let files: Vec<FileView> = meta
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let path = content_path(&resume.save_path, &meta, file, data_root);
            FileView {
                relative: relative_name(&meta, file),
                declared: file.length,
                priority: resume.priority(index),
                observed: stat_file(&path),
            }
        })
        .collect();

    let class = classify::classify(&files, resume.claims_complete(meta.piece_count()));

    Entry {
        id: meta.infohash_hex(),
        name: String::from_utf8_lossy(&meta.name).into_owned(),
        save_path: String::from_utf8_lossy(&resume.save_path).into_owned(),
        bytes: meta.total_length(),
        class,
    }
}

fn unverifiable(id: String, name: String, save_path: String, bytes: u64) -> Entry {
    Entry {
        id,
        name,
        save_path,
        bytes,
        class: classify::Class::Unverifiable,
    }
}

fn stat_file(path: &Path) -> Observed {
    match fs::metadata(path) {
        Ok(meta) => Observed::Present { size: meta.len() },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Observed::Absent,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Observed::Denied,
        Err(_) => Observed::Denied,
    }
}
