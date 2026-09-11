//! Prove a destination client has complete data and has not started seeding.
//!
//! Import leaves torrents paused. Handoff is what starts them. This command
//! is the gate between those steps: piece-complete, not downloading, and
//! still stopped. Tracker reachability and a public listen port belong to
//! handoff / harden — they require announcing, which is dual-seed if the
//! source is still up.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tsync_core::metainfo;
use tsync_qbt::{
    Client, Torrent, is_checking, is_downloading, is_piece_complete, is_seeding, is_stopped,
};

/// How to run a verify.
pub struct Options<'a> {
    /// Staging directory from `tsync rewrite`. Expected hashes come from here.
    pub staging: PathBuf,
    /// Destination client. Read-only.
    pub dest: &'a dyn Client,
    /// Treat a complete, seeding torrent as ready. Default is to fail it —
    /// the source may still be announcing the same hash.
    pub allow_seeding: bool,
    /// Verify only the smallest N staged torrents (same cap as `transfer`).
    pub max_torrents: Option<usize>,
}

/// Why verify could not start.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Staging could not be listed.
    #[error("cannot list staging {path}: {source}")]
    Staging {
        /// Directory that failed.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// A `WebAPI` call failed.
    #[error(transparent)]
    Api(#[from] tsync_qbt::Error),
}

/// Outcome of one verify pass.
#[derive(Clone, Debug)]
pub struct Report {
    /// Complete and stopped (or seeding, if allowed).
    pub ready: usize,
    /// Expected hashes that were not on the destination.
    pub missing: usize,
    /// Still hashing.
    pub checking: usize,
    /// Present but not piece-complete, or pulling from the swarm.
    pub failed: Vec<Failure>,
}

/// One torrent that is not ready.
#[derive(Clone, Debug)]
pub struct Failure {
    /// Infohash or stem.
    pub id: String,
    /// Why it is not ready.
    pub reason: String,
}

impl Report {
    /// Human summary. Names stay off this text.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::from("tsync verify\n\n");
        let _ = writeln!(out, "  ready      {:>5}", self.ready);
        let _ = writeln!(out, "  missing    {:>5}", self.missing);
        let _ = writeln!(out, "  checking   {:>5}", self.checking);
        let _ = writeln!(out, "  failed     {:>5}", self.failed.len());
        if !self.failed.is_empty() {
            let _ = writeln!(out);
            for item in &self.failed {
                let _ = writeln!(out, "  FAIL  {}  {}", short_id(&item.id), item.reason);
            }
        }
        out.push('\n');
        out
    }

    /// True when every expected torrent is ready.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty() && self.missing == 0 && self.checking == 0
    }
}

/// Check staged torrents on `opts.dest`.
///
/// # Errors
///
/// Returns [`Error`] when staging cannot be read or the destination API fails.
pub fn run(opts: &Options<'_>) -> Result<Report, Error> {
    let expected = read_expected(&opts.staging, opts.max_torrents)?;
    let listed = opts.dest.list()?;
    let dest: BTreeMap<String, Torrent> = listed
        .into_iter()
        .map(|torrent| (torrent.hash.to_ascii_lowercase(), torrent))
        .collect();

    let mut ready = 0;
    let mut missing = 0;
    let mut checking = 0;
    let mut failed = Vec::new();

    for hash in &expected {
        match dest.get(hash) {
            None => missing += 1,
            Some(torrent) if is_checking(&torrent.state) => checking += 1,
            Some(torrent) => match classify(torrent, opts.allow_seeding) {
                None => ready += 1,
                Some(reason) => failed.push(Failure {
                    id: hash.clone(),
                    reason,
                }),
            },
        }
    }

    Ok(Report {
        ready,
        missing,
        checking,
        failed,
    })
}

fn classify(torrent: &Torrent, allow_seeding: bool) -> Option<String> {
    if is_downloading(&torrent.state) {
        return Some(format!(
            "downloading ({}); dest must stay paused until handoff",
            torrent.state
        ));
    }
    if !is_piece_complete(torrent) {
        return Some(format!(
            "incomplete (progress {:.3}, {} bytes left)",
            torrent.progress, torrent.amount_left
        ));
    }
    if is_stopped(&torrent.state) {
        return None;
    }
    if is_seeding(&torrent.state) {
        if allow_seeding {
            return None;
        }
        return Some(format!(
            "seeding ({}); stop it, or pass --allow-seeding after handoff",
            torrent.state
        ));
    }
    if torrent.state == "missingFiles" {
        return Some("client reports missingFiles".into());
    }
    Some(format!("unexpected state {}", torrent.state))
}

fn read_expected(
    staging: &Path,
    max_torrents: Option<usize>,
) -> Result<BTreeSet<String>, Error> {
    let listed = fs::read_dir(staging).map_err(|source| Error::Staging {
        path: staging.to_path_buf(),
        source,
    })?;

    let mut items = Vec::new();
    for entry in listed.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("torrent") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(meta) = metainfo::parse(&bytes) else {
            continue;
        };
        items.push((meta.total_length(), meta.infohash_hex()));
    }
    items.sort_by_key(|(bytes, _)| *bytes);
    if let Some(limit) = max_torrents {
        items.truncate(limit);
    }
    Ok(items.into_iter().map(|(_, hash)| hash).collect())
}

fn short_id(id: &str) -> String {
    if id.len() > 12 {
        format!("{}…", &id[..12])
    } else {
        id.to_owned()
    }
}
