//! Add staged torrents to a destination client without starting them.
//!
//! The destination is driven through [`tsync_qbt::Client`]: add paused, stop
//! again, force recheck. A global download-rate circuit breaker is applied for
//! the duration of the run and restored afterwards. The source, when given,
//! is only read — if it is still seeding a hash we are about to add, the
//! whole run is refused.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tsync_core::{metainfo, resume};
use tsync_qbt::{CIRCUIT_BREAKER_BPS, Client, seeding_hashes};

/// How to run an import.
pub struct Options<'a> {
    /// Directory produced by `tsync rewrite`.
    pub staging: PathBuf,
    /// Destination client. This is where torrents are added.
    pub dest: &'a dyn Client,
    /// Source client, used only for the dual-seed guard.
    pub source: Option<&'a dyn Client>,
    /// Allow import when the source API is unavailable.
    pub allow_unverified_source: bool,
    /// Inspect and refuse, but do not add.
    pub dry_run: bool,
    /// Import only the smallest N staged torrents (same cap as `transfer`).
    pub max_torrents: Option<usize>,
}

/// Why import could not start.
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
    /// Dual-seed guard needs a source, and none was given.
    #[error(
        "dual-seed guard needs --source-url (or pass --allow-unverified-source if you accept the risk)"
    )]
    SourceRequired,
    /// Destination is already announcing a hash the source still seeds.
    #[error(
        "destination is already seeding {} torrent(s) the source still announces",
        .0.len()
    )]
    DualSeed(Vec<String>),
    /// A `WebAPI` call failed.
    #[error(transparent)]
    Api(#[from] tsync_qbt::Error),
}

/// Outcome of one import pass.
#[derive(Clone, Debug)]
pub struct Report {
    /// Torrents added and rechecked (or that would be, on a dry run).
    pub imported: usize,
    /// Already present on the destination.
    pub already_present: usize,
    /// Staging entries that were not a torrent/resume pair.
    pub unpaired: usize,
    /// Per-torrent failures after the guards passed.
    pub failed: Vec<Failure>,
    /// True when nothing was written to the destination.
    pub dry_run: bool,
}

/// One torrent that was not imported.
#[derive(Clone, Debug)]
pub struct Failure {
    /// Infohash or stem.
    pub id: String,
    /// Why it failed.
    pub reason: String,
}

impl Report {
    /// Human summary. Names stay off this text.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = if self.dry_run {
            String::from("tsync import — dry run\n\n")
        } else {
            String::from("tsync import\n\n")
        };
        if self.dry_run {
            let _ = writeln!(out, "  imported        —   dry run ({})", self.imported);
        } else {
            let _ = writeln!(out, "  imported   {:>5}", self.imported);
        }
        let _ = writeln!(out, "  already    {:>5}", self.already_present);
        let _ = writeln!(out, "  unpaired   {:>5}", self.unpaired);
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

    /// True when every paired torrent was imported or already present.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Import paired torrents from staging onto `opts.dest`.
///
/// # Errors
///
/// Returns [`Error`] when staging cannot be read, the dual-seed guard fires,
/// the source is missing, or the destination API fails before any add.
pub fn run(opts: &Options<'_>) -> Result<Report, Error> {
    let mut staged = read_staging(&opts.staging)?;
    take_smallest(&mut staged.items, opts.max_torrents);
    let unpaired = staged.unpaired;

    if opts.source.is_none() && !opts.allow_unverified_source {
        return Err(Error::SourceRequired);
    }

    if let Some(source) = opts.source {
        let source_seeding: BTreeSet<String> = seeding_hashes(source)?.into_iter().collect();
        let dest_seeding: BTreeSet<String> = seeding_hashes(opts.dest)?.into_iter().collect();
        let overlap: Vec<String> = staged
            .items
            .iter()
            .filter(|item| source_seeding.contains(&item.hash) && dest_seeding.contains(&item.hash))
            .map(|item| item.hash.clone())
            .collect();
        if !overlap.is_empty() {
            return Err(Error::DualSeed(overlap));
        }
    }

    let dest_hashes: BTreeSet<String> = opts
        .dest
        .list()?
        .into_iter()
        .map(|torrent| torrent.hash.to_ascii_lowercase())
        .collect();

    if opts.dry_run {
        let already_present = staged
            .items
            .iter()
            .filter(|item| dest_hashes.contains(&item.hash))
            .count();
        return Ok(Report {
            imported: staged.items.len() - already_present,
            already_present,
            unpaired,
            failed: Vec::new(),
            dry_run: true,
        });
    }

    let previous = opts.dest.download_limit()?;
    opts.dest.set_download_limit(CIRCUIT_BREAKER_BPS)?;
    let _guard = LimitGuard {
        dest: opts.dest,
        previous,
    };

    let mut imported = 0;
    let mut already_present = 0;
    let mut failed = Vec::new();

    for item in &staged.items {
        if dest_hashes.contains(&item.hash) {
            already_present += 1;
            if let Err(reason) = finish_import(opts.dest, &item.hash) {
                failed.push(Failure {
                    id: item.hash.clone(),
                    reason,
                });
            }
            continue;
        }
        match import_one(opts.dest, item) {
            Ok(()) => imported += 1,
            Err(reason) => failed.push(Failure {
                id: item.hash.clone(),
                reason,
            }),
        }
    }

    Ok(Report {
        imported,
        already_present,
        unpaired,
        failed,
        dry_run: false,
    })
}

fn import_one(dest: &dyn Client, item: &StagedItem) -> Result<(), String> {
    dest.add_paused(
        &item.torrent,
        &format!("{}.torrent", item.hash),
        &item.save_path,
    )
    .map_err(|error| error.to_string())?;
    finish_import(dest, &item.hash)
}

fn finish_import(dest: &dyn Client, hash: &str) -> Result<(), String> {
    dest.stop(hash).map_err(|error| error.to_string())?;
    dest.recheck(hash).map_err(|error| error.to_string())?;
    Ok(())
}

struct LimitGuard<'a> {
    dest: &'a dyn Client,
    previous: i64,
}

impl Drop for LimitGuard<'_> {
    fn drop(&mut self) {
        let _ = self.dest.set_download_limit(self.previous);
    }
}

struct Staged {
    items: Vec<StagedItem>,
    unpaired: usize,
}

struct StagedItem {
    hash: String,
    save_path: String,
    torrent: Vec<u8>,
    bytes: u64,
}

fn take_smallest(items: &mut Vec<StagedItem>, max_torrents: Option<usize>) {
    let Some(limit) = max_torrents else {
        return;
    };
    items.sort_by_key(|item| item.bytes);
    items.truncate(limit);
}

struct Pair {
    torrent: Option<PathBuf>,
    resume: Option<PathBuf>,
}

fn read_staging(staging: &Path) -> Result<Staged, Error> {
    let listed = fs::read_dir(staging).map_err(|source| Error::Staging {
        path: staging.to_path_buf(),
        source,
    })?;

    let mut pairs: BTreeMap<String, Pair> = BTreeMap::new();
    for entry in listed.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(ext) = path.extension().and_then(|name| name.to_str()) else {
            continue;
        };
        let slot = pairs.entry(stem.to_ascii_lowercase()).or_insert(Pair {
            torrent: None,
            resume: None,
        });
        match ext {
            "torrent" => slot.torrent = Some(path),
            "fastresume" => slot.resume = Some(path),
            _ => {}
        }
    }

    let mut items = Vec::new();
    let mut unpaired = 0;
    for (stem, pair) in pairs {
        match (pair.torrent, pair.resume) {
            (Some(torrent_path), Some(resume_path)) => {
                match load_item(&stem, &torrent_path, &resume_path) {
                    Ok(item) => items.push(item),
                    Err(()) => unpaired += 1,
                }
            }
            _ => unpaired += 1,
        }
    }

    Ok(Staged { items, unpaired })
}

fn load_item(stem: &str, torrent_path: &Path, resume_path: &Path) -> Result<StagedItem, ()> {
    let torrent = fs::read(torrent_path).map_err(|_| ())?;
    let resume_bytes = fs::read(resume_path).map_err(|_| ())?;
    let meta = metainfo::parse(&torrent).map_err(|_| ())?;
    let parsed = resume::parse(&resume_bytes).map_err(|_| ())?;
    let hash = meta.infohash_hex();
    if hash != stem {
        return Err(());
    }
    Ok(StagedItem {
        hash,
        save_path: String::from_utf8_lossy(&parsed.save_path).into_owned(),
        torrent,
        bytes: meta.total_length(),
    })
}

fn short_id(id: &str) -> String {
    if id.len() > 12 {
        format!("{}…", &id[..12])
    } else {
        id.to_owned()
    }
}
