//! Prove a destination client has complete data and has not started seeding.
//!
//! Import leaves torrents paused. Handoff is what starts them. This command
//! is the gate between those steps: piece-complete, not downloading, and
//! still stopped. `--recheck` asks dest to hash again and does **not** stop
//! afterwards — a stop mid-check cancels the hash on qBittorrent 5.x.
//! Tracker reachability and a public listen port belong to handoff / harden
//! — they require announcing, which is dual-seed if the source is still up.

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
    /// Destination client. Read-only unless [`Self::recheck`] is set.
    pub dest: &'a dyn Client,
    /// Treat a complete, seeding torrent as ready. Default is to fail it —
    /// the source may still be announcing the same hash.
    pub allow_seeding: bool,
    /// Verify only the smallest N staged torrents (same cap as `transfer`).
    pub max_torrents: Option<usize>,
    /// Force-recheck incomplete, stopped hashes. Skips torrents that are
    /// already hashing, complete, or actively downloading. Never starts dest,
    /// and never stops it (stop cancels an in-flight recheck).
    pub recheck: bool,
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
    /// Incomplete stopped hashes this pass asked dest to recheck.
    pub rechecked: usize,
}

/// Live dest hash pass: counts plus per-torrent bars. Names stay off.
#[derive(Clone, Debug)]
pub struct Watch {
    /// Same classification `verify` prints at the end.
    pub report: Report,
    /// Bytes dest has hashed or already accepted.
    pub hashed_bytes: u64,
    /// Declared size of staged torrents dest knows about.
    pub total_bytes: u64,
    /// Torrents dest is hashing right now, highest progress first.
    pub hashing: Vec<Hashing>,
}

/// One torrent dest is currently hashing.
#[derive(Clone, Debug, PartialEq)]
pub struct Hashing {
    /// Infohash or stem.
    pub id: String,
    /// 0.0–1.0 as the client reports it.
    pub progress: f64,
    /// Declared size in bytes.
    pub size: u64,
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
        if self.rechecked > 0 {
            let _ = writeln!(out, "  rechecked  {:>5}", self.rechecked);
        }
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
    Ok(watch(opts)?.report)
}

/// Same pass as [`run`], plus byte totals and the torrents dest is hashing.
///
/// # Errors
///
/// Returns [`Error`] when staging cannot be read or the destination API fails.
pub fn watch(opts: &Options<'_>) -> Result<Watch, Error> {
    let expected = expected_hashes(&opts.staging, opts.max_torrents)?;
    let listed = opts.dest.list()?;
    let dest: BTreeMap<String, Torrent> = listed
        .into_iter()
        .map(|torrent| (torrent.hash.to_ascii_lowercase(), torrent))
        .collect();

    let mut kicked = BTreeSet::new();
    if opts.recheck {
        for hash in hashes_to_recheck(&expected, &dest) {
            opts.dest.recheck(hash)?;
            kicked.insert(hash.to_owned());
        }
    }
    let rechecked = kicked.len();

    let mut ready = 0;
    let mut missing = 0;
    let mut checking = 0;
    let mut failed = Vec::new();
    let mut hashed_bytes = 0;
    let mut total_bytes = 0;
    let mut hashing = Vec::new();

    for hash in &expected {
        match dest.get(hash) {
            None => missing += 1,
            Some(torrent) => {
                let (hashed, total) = hashed_and_total(torrent);
                hashed_bytes += hashed;
                total_bytes += total;
                if is_checking(&torrent.state) || kicked.contains(hash) {
                    checking += 1;
                    hashing.push(Hashing {
                        id: hash.clone(),
                        progress: torrent.progress,
                        size: total,
                    });
                } else {
                    match classify(torrent, opts.allow_seeding) {
                        None => ready += 1,
                        Some(reason) => failed.push(Failure {
                            id: hash.clone(),
                            reason,
                        }),
                    }
                }
            }
        }
    }

    hashing.sort_by(|left, right| {
        right
            .progress
            .partial_cmp(&left.progress)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });

    Ok(Watch {
        report: Report {
            ready,
            missing,
            checking,
            failed,
            rechecked,
        },
        hashed_bytes,
        total_bytes,
        hashing,
    })
}

impl Watch {
    /// Compact live frame. No per-torrent FAIL dump — that belongs on the
    /// final [`Report::render`] after hashing stops.
    #[must_use]
    pub fn render(&self) -> String {
        self.render_rate(None)
    }

    /// Same as [`Self::render`], with an optional hash rate from the caller.
    #[must_use]
    pub fn render_rate(&self, bytes_per_sec: Option<u64>) -> String {
        let mut out = String::from("tsync verify — dest recheck\n\n");
        let total = self.report.ready
            + self.report.checking
            + self.report.failed.len()
            + self.report.missing;
        let torrent_numer = u64::try_from(self.report.ready).unwrap_or(u64::MAX);
        let torrent_denom = u64::try_from(total).unwrap_or(u64::MAX);
        let _ = writeln!(
            out,
            "  torrents  {}  {:>3}/{}  ready",
            ratio_bar(torrent_numer, torrent_denom, 24),
            self.report.ready,
            total
        );
        let _ = writeln!(
            out,
            "  bytes     {}  {} / {}",
            ratio_bar(self.hashed_bytes, self.total_bytes, 24),
            format_bytes(self.hashed_bytes),
            format_bytes(self.total_bytes)
        );
        if let Some(rate) = bytes_per_sec.filter(|rate| *rate > 0) {
            let _ = writeln!(out, "  rate      {}", format_rate(rate));
            if self.hashed_bytes < self.total_bytes && rate > 0 {
                let left = (self.total_bytes - self.hashed_bytes) / rate;
                let _ = writeln!(out, "  eta       {}", format_secs(left));
            }
        }
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  checking {:>5}   queued {:>5}   missing {:>5}",
            self.report.checking,
            self.report.failed.len(),
            self.report.missing
        );
        let _ = writeln!(out);
        if self.hashing.is_empty() {
            let _ = writeln!(out, "  (no torrent is hashing right now)");
        } else {
            for item in self.hashing.iter().take(8) {
                let _ = writeln!(
                    out,
                    "  {:<14} {}  {:>3.0}%  {}",
                    short_id(&item.id),
                    ratio_bar(progress_permille(item.progress), 1000, 20),
                    item.progress * 100.0,
                    format_bytes(item.size)
                );
            }
            if self.hashing.len() > 8 {
                let _ = writeln!(out, "  … {} more hashing", self.hashing.len() - 8);
            }
        }
        out.push('\n');
        out
    }
}

fn hashed_and_total(torrent: &Torrent) -> (u64, u64) {
    let permille = progress_permille(torrent.progress);
    let total = if torrent.size > 0 {
        torrent.size
    } else if permille >= 1000 {
        torrent.completed
    } else if permille == 0 {
        torrent.amount_left
    } else {
        torrent.amount_left.saturating_mul(1000) / (1000 - permille)
    };
    let hashed = if torrent.completed > 0 {
        torrent.completed.min(total)
    } else if total == 0 {
        0
    } else {
        total.saturating_mul(permille) / 1000
    };
    (hashed, total)
}

/// qBittorrent reports `progress` as 0.0–1.0. Thousandths keep bar math
/// in integers.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn progress_permille(progress: f64) -> u64 {
    (progress.clamp(0.0, 1.0) * 1000.0).round() as u64
}

fn ratio_bar(numer: u64, denom: u64, width: usize) -> String {
    let width = width.max(1);
    let filled = if denom == 0 {
        0
    } else {
        let width_u = u32::try_from(width).unwrap_or(u32::MAX);
        let cells = (u128::from(numer) * u128::from(width_u)) / u128::from(denom);
        usize::try_from(cells).unwrap_or(width).min(width)
    };
    format!("[{}{}]", "#".repeat(filled), "-".repeat(width - filled))
}

fn format_bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if n >= GIB {
        let whole = n / GIB;
        let frac = (n % GIB) * 100 / GIB;
        format!("{whole}.{frac:02} GiB")
    } else if n >= MIB {
        format!("{} MiB", n / MIB)
    } else if n >= KIB {
        format!("{} KiB", n / KIB)
    } else {
        format!("{n} B")
    }
}

fn format_rate(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

fn format_secs(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// Incomplete, stopped hashes. Already-checking, complete, and live
/// downloads are left alone.
fn hashes_to_recheck<'a>(
    expected: &'a BTreeSet<String>,
    dest: &'a BTreeMap<String, Torrent>,
) -> impl Iterator<Item = &'a str> {
    expected.iter().filter_map(|hash| {
        let torrent = dest.get(hash)?;
        if is_stopped(&torrent.state) && !is_piece_complete(torrent) {
            Some(hash.as_str())
        } else {
            None
        }
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

/// Staged infohashes, smallest first, then truncated by `max_torrents`.
///
/// # Errors
///
/// Returns [`Error::Staging`] when the directory cannot be listed.
pub fn expected_hashes(
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
