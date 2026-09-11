//! Pull torrent data onto this machine. Never add it to a client.
//!
//! Fetch is a file copy that knows about torrents. The remote keeps seeding.
//! A fetched tree that is never imported cannot announce, so handoff does
//! not apply. Importing a fetched set is a separate command.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tsync_audit::paths::content_path;
use tsync_core::{metainfo, resume};
use tsync_qbt::{Client, Torrent, TorrentFile, is_piece_complete};

/// How to treat torrents that are not 100% on the remote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PartialMode {
    /// Skip the torrent and report it. The default.
    #[default]
    Skip,
    /// Copy only files the remote reports as complete.
    CompleteFiles,
    /// Copy every file, including in-flight ones.
    All,
}

impl std::str::FromStr for PartialMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "skip" => Ok(Self::Skip),
            "complete-files" => Ok(Self::CompleteFiles),
            "all" => Ok(Self::All),
            other => Err(format!(
                "unknown --partial {other} (skip, complete-files, or all)"
            )),
        }
    }
}

/// How to run a fetch.
pub struct Options<'a> {
    /// Staging directory from `tsync rewrite`. Hashes and dest save paths.
    pub staging: PathBuf,
    /// Where bytes live now. Local path or `host:/path`.
    pub from: String,
    /// Local directory that receives the files. Never a client.
    pub to: String,
    /// Client-visible save root (`/data`), used to make relative paths.
    pub save_root: String,
    /// The remote client. Read-only. Used to skip incomplete torrents.
    pub remote: &'a dyn Client,
    /// Incomplete-torrent policy.
    pub partial: PartialMode,
    /// Classify only; do not copy.
    pub dry_run: bool,
    /// Fetch only the smallest N staged torrents.
    pub max_torrents: Option<usize>,
}

/// Why fetch could not start.
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
    /// `--to` is empty or `/`.
    #[error("fetch destination is empty or /")]
    BadDestination,
    /// A `WebAPI` call failed.
    #[error(transparent)]
    Api(#[from] tsync_qbt::Error),
    /// rsync is missing or the copy failed before any torrent finished.
    #[error(transparent)]
    Transfer(#[from] tsync_transfer::Error),
}

/// Outcome of one fetch pass.
#[derive(Clone, Debug)]
pub struct Report {
    /// Torrents whose wanted files were copied (or would be, on a dry run).
    pub fetched: usize,
    /// Incomplete on the remote and skipped.
    pub skipped: usize,
    /// Per-torrent failures.
    pub failed: Vec<Failure>,
    /// True when nothing was written.
    pub dry_run: bool,
}

/// One torrent that was not fetched.
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
            String::from("tsync fetch — dry run\n\n")
        } else {
            String::from("tsync fetch\n\n")
        };
        let _ = writeln!(out, "  fetched    {:>5}", self.fetched);
        let _ = writeln!(out, "  skipped    {:>5}", self.skipped);
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

    /// True when nothing failed.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

struct Staged {
    hash: String,
    save_path: String,
    meta: metainfo::Metainfo,
    bytes: u64,
}

/// Copy staged torrents from `opts.from` to `opts.to`. Never imports.
///
/// # Errors
///
/// Returns [`Error`] when staging cannot be read, the destination is unusable,
/// or the remote API fails before any copy.
pub fn run(opts: &Options<'_>) -> Result<Report, Error> {
    if opts.to.is_empty() || opts.to == "/" {
        return Err(Error::BadDestination);
    }

    let mut staged = read_staging(&opts.staging)?;
    staged.sort_by_key(|item| item.bytes);
    if let Some(limit) = opts.max_torrents {
        staged.truncate(limit);
    }

    let remote: BTreeMap<String, Torrent> = opts
        .remote
        .list()?
        .into_iter()
        .map(|torrent| (torrent.hash.to_ascii_lowercase(), torrent))
        .collect();

    let mut fetched = 0;
    let mut skipped = 0;
    let mut failed = Vec::new();

    for item in &staged {
        match select_files(opts, item, remote.get(&item.hash)) {
            Select::Skip => skipped += 1,
            Select::Fail(reason) => failed.push(Failure {
                id: item.hash.clone(),
                reason,
            }),
            Select::Files(files) => {
                if opts.dry_run {
                    fetched += 1;
                    continue;
                }
                match tsync_transfer::copy_relatives(&opts.from, &opts.to, &files) {
                    Ok(()) => fetched += 1,
                    Err(error) => failed.push(Failure {
                        id: item.hash.clone(),
                        reason: error.to_string(),
                    }),
                }
            }
        }
    }

    Ok(Report {
        fetched,
        skipped,
        failed,
        dry_run: opts.dry_run,
    })
}

enum Select {
    Skip,
    Fail(String),
    Files(Vec<PathBuf>),
}

fn select_files(opts: &Options<'_>, item: &Staged, remote: Option<&Torrent>) -> Select {
    let Some(remote) = remote else {
        return Select::Fail("missing on remote".into());
    };
    let complete = is_piece_complete(remote);
    if !complete && opts.partial == PartialMode::Skip {
        return Select::Skip;
    }

    let wanted = if !complete && opts.partial == PartialMode::CompleteFiles {
        match opts.remote.files(&item.hash) {
            Ok(files) => Some(files),
            Err(error) => return Select::Fail(error.to_string()),
        }
    } else {
        None
    };

    let files = relatives(
        &item.save_path,
        &opts.save_root,
        &item.meta,
        wanted.as_deref(),
    );
    if files.is_empty() {
        return Select::Fail("no complete files to copy".into());
    }
    Select::Files(files)
}

fn relatives(
    save_path: &str,
    save_root: &str,
    meta: &metainfo::Metainfo,
    wanted: Option<&[TorrentFile]>,
) -> Vec<PathBuf> {
    let root = Path::new(save_root.trim_end_matches('/'));
    let mut out = Vec::new();
    for file in &meta.files {
        if let Some(remote_files) = wanted
            && !file_complete(remote_files, meta, file)
        {
            continue;
        }
        let abs = content_path(save_path.as_bytes(), meta, file, None);
        if let Ok(relative) = abs.strip_prefix(root) {
            out.push(relative.to_path_buf());
        }
    }
    out
}

fn file_complete(
    remote: &[TorrentFile],
    meta: &metainfo::Metainfo,
    file: &metainfo::ContentFile,
) -> bool {
    let rel = relative_posix(meta, file);
    remote
        .iter()
        .any(|entry| entry.progress >= 1.0 && (entry.name == rel || entry.name.ends_with(&rel)))
}

fn relative_posix(meta: &metainfo::Metainfo, file: &metainfo::ContentFile) -> String {
    if meta.multi_file {
        let mut parts = vec![String::from_utf8_lossy(&meta.name).into_owned()];
        parts.extend(
            file.path
                .iter()
                .map(|component| String::from_utf8_lossy(component).into_owned()),
        );
        parts.join("/")
    } else {
        String::from_utf8_lossy(&meta.name).into_owned()
    }
}

fn read_staging(staging: &Path) -> Result<Vec<Staged>, Error> {
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
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let Ok(torrent) = fs::read(&path) else {
            continue;
        };
        let Ok(meta) = metainfo::parse(&torrent) else {
            continue;
        };
        let resume_path = staging.join(format!("{stem}.fastresume"));
        let Ok(resume_bytes) = fs::read(&resume_path) else {
            continue;
        };
        let Ok(parsed) = resume::parse(&resume_bytes) else {
            continue;
        };
        let hash = meta.infohash_hex();
        if hash != stem {
            continue;
        }
        items.push(Staged {
            hash,
            save_path: String::from_utf8_lossy(&parsed.save_path).into_owned(),
            bytes: meta.total_length(),
            meta,
        });
    }
    Ok(items)
}

fn short_id(id: &str) -> String {
    if id.len() > 12 {
        format!("{}…", &id[..12])
    } else {
        id.to_owned()
    }
}
