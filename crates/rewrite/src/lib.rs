//! Write rewritten `.fastresume` files into a staging directory.
//!
//! Originals are never opened for write. Each resume is decoded, proven to
//! round-trip byte-identically, then has only its path keys replaced. A file
//! that fails that gate is skipped, not normalized.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tsync_audit::Options as AuditOptions;
use tsync_core::resume;
use tsync_plan::{Plan, Planned};

/// How to run a rewrite.
#[derive(Clone, Debug)]
pub struct Options {
    /// Source `BT_backup`. Read-only.
    pub bt_backup: PathBuf,
    /// Directory that will receive copied `.torrent` files and new resumes.
    pub staging: PathBuf,
    /// Destination data root for the rewritten paths.
    pub dest: String,
    /// Batch budget forwarded to plan.
    pub budget: u64,
    /// Relocate fixture save paths, same as audit.
    pub data_root: Option<PathBuf>,
}

/// Why rewrite could not finish (or could not start).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Audit could not read the backup directory.
    #[error(transparent)]
    Audit(#[from] tsync_audit::Error),
    /// The plan could not be derived.
    #[error(transparent)]
    Plan(#[from] tsync_plan::Error),
    /// Staging would overwrite the source backup.
    #[error("staging directory must not be the source BT_backup")]
    StagingIsSource,
    /// qBittorrent looks like it is running; resume data in memory would
    /// overwrite anything we did, and we refuse to race it.
    #[error("qBittorrent lockfile present at {0}; stop the client and retry")]
    ClientRunning(PathBuf),
    /// Staging directory could not be created.
    #[error("cannot create staging {path}: {source}")]
    StagingIo {
        /// Directory we tried to create.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
}

/// Outcome of rewriting one library into staging.
#[derive(Clone, Debug)]
pub struct Report {
    /// The plan that was applied.
    pub plan: Plan,
    /// Torrents written to staging.
    pub written: usize,
    /// Eligible torrents that failed the round-trip gate or I/O.
    pub failed: Vec<Failure>,
    /// Staging directory used.
    pub staging: PathBuf,
}

/// One torrent that was not written.
#[derive(Clone, Debug)]
pub struct Failure {
    /// Infohash or stem.
    pub id: String,
    /// Why it was refused.
    pub reason: String,
}

impl Report {
    /// Human summary.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = format!("tsync rewrite — staging {}\n\n", self.staging.display());
        let _ = writeln!(out, "  written    {:>5}", self.written);
        let _ = writeln!(out, "  failed     {:>5}", self.failed.len());
        let _ = writeln!(
            out,
            "  excluded   {:>5}   (from plan, not attempted)",
            self.plan.excluded.len()
        );
        if !self.failed.is_empty() {
            let _ = writeln!(out);
            for item in &self.failed {
                let _ = writeln!(out, "  FAIL  {}  {}", short_id(&item.id), item.reason);
            }
        }
        out.push('\n');
        out
    }

    /// True when every eligible torrent was written.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty() && self.written == self.plan.eligible_count()
    }
}

/// Rewrites planned torrents into `opts.staging`.
///
/// # Errors
///
/// Returns [`Error`] when the source cannot be read, staging is the source,
/// the client appears to be running, or staging cannot be created. Per-torrent
/// failures are recorded on the report instead of aborting the run.
pub fn run(opts: &Options) -> Result<Report, Error> {
    refuse_if_source(opts)?;
    refuse_if_client_running(&opts.bt_backup)?;

    let manifest = tsync_audit::run(&AuditOptions {
        bt_backup: opts.bt_backup.clone(),
        data_root: opts.data_root.clone(),
    })?;
    let plan = tsync_plan::build(&manifest, &opts.dest, opts.budget)?;

    fs::create_dir_all(&opts.staging).map_err(|source| Error::StagingIo {
        path: opts.staging.clone(),
        source,
    })?;

    let mut written = 0;
    let mut failed = Vec::new();

    for item in plan.batches.iter().flat_map(|batch| batch.torrents.iter()) {
        match write_one(&opts.bt_backup, &opts.staging, item) {
            Ok(()) => written += 1,
            Err(reason) => failed.push(Failure {
                id: item.entry.id.clone(),
                reason,
            }),
        }
    }

    Ok(Report {
        plan,
        written,
        failed,
        staging: opts.staging.clone(),
    })
}

fn write_one(backup: &Path, staging: &Path, item: &Planned) -> Result<(), String> {
    let stem = &item.entry.id;
    let src_torrent = backup.join(format!("{stem}.torrent"));
    let src_resume = backup.join(format!("{stem}.fastresume"));
    let dst_torrent = staging.join(format!("{stem}.torrent"));
    let dst_resume = staging.join(format!("{stem}.fastresume"));

    let resume_bytes = fs::read(&src_resume).map_err(|error| format!("read resume: {error}"))?;
    let rewritten = resume::rewrite_save_path(&resume_bytes, item.dest_path.as_bytes())
        .map_err(|error| error.to_string())?;

    fs::copy(&src_torrent, &dst_torrent).map_err(|error| format!("copy torrent: {error}"))?;
    fs::write(&dst_resume, rewritten).map_err(|error| format!("write resume: {error}"))?;
    Ok(())
}

fn refuse_if_source(opts: &Options) -> Result<(), Error> {
    let Ok(backup) = fs::canonicalize(&opts.bt_backup) else {
        return Ok(());
    };
    if let Ok(staging) = fs::canonicalize(&opts.staging)
        && staging == backup
    {
        return Err(Error::StagingIsSource);
    }
    Ok(())
}

fn refuse_if_client_running(bt_backup: &Path) -> Result<(), Error> {
    let Some(parent) = bt_backup.parent() else {
        return Ok(());
    };
    let lock = parent.join("lockfile");
    if lock.exists() {
        return Err(Error::ClientRunning(lock));
    }
    Ok(())
}

fn short_id(id: &str) -> String {
    if id.len() > 12 {
        format!("{}…", &id[..12])
    } else {
        id.to_owned()
    }
}
