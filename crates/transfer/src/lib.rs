//! Copy torrent content according to a plan.
//!
//! Source files are read-only. Destination layout follows the rewritten save
//! paths. Files are copied by the paths in the torrent, not by walking the
//! source directory — so a process that can `stat` a file can copy it, even
//! when macOS TCC refuses `opendir` on the parent.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::PathBuf;

use tsync_audit::Options as AuditOptions;
use tsync_audit::paths::content_path;
use tsync_core::metainfo;
use tsync_plan::{Plan, Planned};

/// How to run a transfer.
#[derive(Clone, Debug)]
pub struct Options {
    /// Source `BT_backup`. Read-only.
    pub bt_backup: PathBuf,
    /// Destination data root — mapping target and local copy destination.
    pub dest: String,
    /// Batch budget forwarded to plan.
    pub budget: u64,
    /// Relocate fixture save paths, same as audit.
    pub data_root: Option<PathBuf>,
    /// Stop before including a torrent that would exceed this many bytes.
    pub max_bytes: Option<u64>,
    /// Stop after this many torrents.
    pub max_torrents: Option<usize>,
    /// Plan and select only; do not copy.
    pub dry_run: bool,
}

/// Why a transfer could not start.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Audit could not read the backup directory.
    #[error(transparent)]
    Audit(#[from] tsync_audit::Error),
    /// The plan could not be derived.
    #[error(transparent)]
    Plan(#[from] tsync_plan::Error),
    /// Destination root is empty or `/`.
    #[error("destination root is empty or /")]
    BadDestination,
}

/// Outcome of one transfer pass.
#[derive(Clone, Debug)]
pub struct Report {
    /// The (possibly capped) plan that was applied.
    pub plan: Plan,
    /// Eligible torrents left out by the cap.
    pub deferred: usize,
    /// Bytes left out by the cap.
    pub deferred_bytes: u64,
    /// Torrents whose wanted files were copied.
    pub copied: usize,
    /// Selected torrents that failed.
    pub failed: Vec<Failure>,
    /// True when nothing was written.
    pub dry_run: bool,
    /// Destination root.
    pub dest: String,
}

/// One torrent that was not copied.
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
            format!("tsync transfer — dry run → {}\n\n", self.dest)
        } else {
            format!("tsync transfer — {}\n\n", self.dest)
        };

        let _ = writeln!(
            out,
            "  selected    {:>5}   {}",
            self.plan.eligible_count(),
            format_bytes(self.plan.eligible_bytes())
        );
        if self.deferred > 0 {
            let _ = writeln!(
                out,
                "  deferred    {:>5}   {}   (over cap, not attempted)",
                self.deferred,
                format_bytes(self.deferred_bytes)
            );
        }
        if self.dry_run {
            let _ = writeln!(out, "  copied          —   dry run");
        } else {
            let _ = writeln!(out, "  copied     {:>5}", self.copied);
            let _ = writeln!(out, "  failed     {:>5}", self.failed.len());
        }
        let _ = writeln!(
            out,
            "  excluded    {:>5}   (from plan, not attempted)",
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

    /// True when every selected torrent was copied (or this was a dry run).
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.dry_run || (self.failed.is_empty() && self.copied == self.plan.eligible_count())
    }
}

/// Copies planned torrents to `opts.dest`.
///
/// # Errors
///
/// Returns [`Error`] when the source cannot be read or the destination is
/// unusable. Per-torrent copy failures are recorded on the report instead of
/// aborting the run.
pub fn run(opts: &Options) -> Result<Report, Error> {
    if opts.dest.is_empty() || opts.dest == "/" {
        return Err(Error::BadDestination);
    }

    let manifest = tsync_audit::run(&AuditOptions {
        bt_backup: opts.bt_backup.clone(),
        data_root: opts.data_root.clone(),
    })?;
    let full = tsync_plan::build(&manifest, &opts.dest, opts.budget)?;
    let all_count = full.eligible_count();
    let all_bytes = full.eligible_bytes();
    let plan = full.take_smallest(opts.max_bytes, opts.max_torrents);
    let deferred = all_count.saturating_sub(plan.eligible_count());
    let deferred_bytes = all_bytes.saturating_sub(plan.eligible_bytes());

    if opts.dry_run {
        return Ok(Report {
            plan,
            deferred,
            deferred_bytes,
            copied: 0,
            failed: Vec::new(),
            dry_run: true,
            dest: opts.dest.clone(),
        });
    }

    let mut copied = 0;
    let mut failed = Vec::new();

    for item in plan.batches.iter().flat_map(|batch| batch.torrents.iter()) {
        match copy_one(opts, item) {
            Ok(()) => copied += 1,
            Err(reason) => failed.push(Failure {
                id: item.entry.id.clone(),
                reason,
            }),
        }
    }

    Ok(Report {
        plan,
        deferred,
        deferred_bytes,
        copied,
        failed,
        dry_run: false,
        dest: opts.dest.clone(),
    })
}

fn copy_one(opts: &Options, item: &Planned) -> Result<(), String> {
    let torrent_path = opts.bt_backup.join(format!("{}.torrent", item.entry.id));
    let bytes = fs::read(&torrent_path).map_err(|error| format!("read torrent: {error}"))?;
    let meta = metainfo::parse(&bytes).map_err(|error| error.to_string())?;

    let mut copied_files = 0;
    for file in &meta.files {
        let source = content_path(
            item.entry.save_path.as_bytes(),
            &meta,
            file,
            opts.data_root.as_deref(),
        );
        if !source.exists() {
            continue;
        }
        let dest = content_path(item.dest_path.as_bytes(), &meta, file, None);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|error| copy_error(&error))?;
        }
        fs::copy(&source, &dest).map_err(|error| copy_error(&error))?;
        copied_files += 1;
    }

    if copied_files == 0 {
        return Err("no source files could be copied".to_owned());
    }
    Ok(())
}

fn copy_error(error: &io::Error) -> String {
    if error.kind() == io::ErrorKind::PermissionDenied {
        "permission denied reading source (macOS Full Disk Access?)".to_owned()
    } else {
        format!("copy: {error}")
    }
}

fn short_id(id: &str) -> String {
    if id.len() > 12 {
        format!("{}…", &id[..12])
    } else {
        id.to_owned()
    }
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
