//! Run the migration pipeline as one command.
//!
//! Order is fixed: rewrite → transfer → import → verify → (optional) handoff.
//! Handoff is off unless asked for, so dest stays paused after a migrate.

use std::fmt::Write as _;
use std::path::PathBuf;

use tsync_qbt::Client;

/// First step to run. Earlier steps are skipped (resume a stopped migrate).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    /// Write staging (default start).
    #[default]
    Rewrite,
    /// Copy payload.
    Transfer,
    /// Add paused on dest.
    Import,
    /// Prove dest is complete and still stopped.
    Verify,
    /// Stop source, start dest.
    Handoff,
}

impl std::str::FromStr for Step {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "rewrite" => Ok(Self::Rewrite),
            "transfer" => Ok(Self::Transfer),
            "import" => Ok(Self::Import),
            "verify" => Ok(Self::Verify),
            "handoff" => Ok(Self::Handoff),
            other => Err(format!(
                "unknown --from-step {other} (rewrite, transfer, import, verify, handoff)"
            )),
        }
    }
}

/// How to run a migrate.
#[allow(clippy::struct_excessive_bools)]
pub struct Options<'a> {
    /// Source `BT_backup`.
    pub bt_backup: PathBuf,
    /// Staging directory for rewritten resumes.
    pub staging: PathBuf,
    /// Destination save-path root (`/data`).
    pub dest: String,
    /// Where bytes are written. Local path or `host:/path`.
    pub rsync_to: Option<String>,
    /// Relocate fixture save paths, same as audit.
    pub data_root: Option<PathBuf>,
    /// Soft batch budget in bytes.
    pub budget: u64,
    /// Destination client.
    pub dest_client: &'a dyn Client,
    /// Source client, for import guard and handoff.
    pub source_client: Option<&'a dyn Client>,
    /// Allow import when the source API is unavailable.
    pub allow_unverified_source: bool,
    /// Run handoff after a complete verify. Default is to stop at verify.
    pub handoff: bool,
    /// Handoff `--confirm` when the source client is omitted.
    pub confirm: bool,
    /// Resume from this step.
    pub from_step: Step,
    /// Dry-run transfer, import, and handoff. Rewrite still writes staging.
    pub dry_run: bool,
    /// Cap every step to the smallest N torrents.
    pub max_torrents: Option<usize>,
}

/// Why migrate could not start.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Rewrite failed.
    #[error(transparent)]
    Rewrite(#[from] tsync_rewrite::Error),
    /// Transfer failed to start.
    #[error(transparent)]
    Transfer(#[from] tsync_transfer::Error),
    /// Import failed to start.
    #[error(transparent)]
    Import(#[from] tsync_import::Error),
    /// Verify failed to start.
    #[error(transparent)]
    Verify(#[from] tsync_verify::Error),
    /// Handoff failed to start.
    #[error(transparent)]
    Handoff(#[from] tsync_handoff::Error),
}

/// Outcome of one migrate pass.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Last step that ran.
    pub stopped_at: &'static str,
    /// Transfer copied this many torrents.
    pub transferred: usize,
    /// Import added this many torrents.
    pub imported: usize,
    /// Verify ready count.
    pub ready: usize,
    /// Handoff started this many torrents.
    pub started: usize,
    /// True when a later step was skipped because an earlier one was incomplete.
    pub halted: bool,
    /// Dry run.
    pub dry_run: bool,
}

impl Report {
    /// Human summary.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = if self.dry_run {
            String::from("tsync migrate — dry run\n\n")
        } else {
            String::from("tsync migrate\n\n")
        };
        let _ = writeln!(out, "  stopped at   {}", self.stopped_at);
        let _ = writeln!(out, "  transferred  {:>5}", self.transferred);
        let _ = writeln!(out, "  imported     {:>5}", self.imported);
        let _ = writeln!(out, "  ready        {:>5}", self.ready);
        let _ = writeln!(out, "  started      {:>5}", self.started);
        if self.halted {
            let _ = writeln!(out, "\n  halted before a later step; dest was not started.");
        }
        out.push('\n');
        out
    }

    /// True when the requested pipeline finished without a halt.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.halted
    }
}

/// Run rewrite → transfer → import → verify → optional handoff.
///
/// # Errors
///
/// Returns [`Error`] when a step cannot start. Per-torrent failures halt
/// later steps and are reported as [`Report::halted`].
pub fn run(opts: &Options<'_>) -> Result<Report, Error> {
    let mut report = Report {
        dry_run: opts.dry_run,
        ..Report::default()
    };

    if opts.from_step <= Step::Rewrite {
        tsync_rewrite::run(&tsync_rewrite::Options {
            bt_backup: opts.bt_backup.clone(),
            staging: opts.staging.clone(),
            dest: opts.dest.clone(),
            budget: opts.budget,
            data_root: opts.data_root.clone(),
        })?;
        report.stopped_at = "rewrite";
    }

    if opts.from_step <= Step::Transfer {
        let transfer = tsync_transfer::run(&tsync_transfer::Options {
            bt_backup: opts.bt_backup.clone(),
            dest: opts.dest.clone(),
            rsync_to: opts.rsync_to.clone(),
            budget: opts.budget,
            data_root: opts.data_root.clone(),
            max_bytes: None,
            max_torrents: opts.max_torrents,
            dry_run: opts.dry_run,
        })?;
        report.transferred = transfer.copied;
        report.stopped_at = "transfer";
        if !transfer.is_complete() {
            report.halted = true;
            return Ok(report);
        }
    }

    if opts.from_step <= Step::Import {
        let import = tsync_import::run(&tsync_import::Options {
            staging: opts.staging.clone(),
            dest: opts.dest_client,
            source: opts.source_client,
            allow_unverified_source: opts.allow_unverified_source,
            dry_run: opts.dry_run,
            max_torrents: opts.max_torrents,
        })?;
        report.imported = import.imported;
        report.stopped_at = "import";
        if !import.is_complete() {
            report.halted = true;
            return Ok(report);
        }
    }

    if opts.from_step <= Step::Verify {
        let verify = tsync_verify::run(&tsync_verify::Options {
            staging: opts.staging.clone(),
            dest: opts.dest_client,
            allow_seeding: false,
            max_torrents: opts.max_torrents,
            recheck: false,
        })?;
        report.ready = verify.ready;
        report.stopped_at = "verify";
        if !verify.is_complete() {
            report.halted = true;
            return Ok(report);
        }
    }

    if opts.handoff && opts.from_step <= Step::Handoff {
        let handoff = tsync_handoff::run(&tsync_handoff::Options {
            staging: opts.staging.clone(),
            dest: opts.dest_client,
            source: opts.source_client,
            confirm: opts.confirm,
            dry_run: opts.dry_run,
            max_torrents: opts.max_torrents,
        })?;
        report.started = handoff.started;
        report.stopped_at = "handoff";
        if !handoff.is_complete() {
            report.halted = true;
        }
    }

    Ok(report)
}
