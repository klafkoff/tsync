//! Stop the source, then start the destination. Never the reverse.
//!
//! Import leaves dest paused. Verify proves it is piece-complete. Handoff is
//! the only command that starts dest, and only after the source is silent for
//! that infohash. A brief gap where neither announces is deliberate.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use tsync_qbt::{
    CIRCUIT_BREAKER_BPS, Client, Torrent, is_checking, is_downloading, is_piece_complete,
    is_seeding, is_stopped,
};
use tsync_verify::expected_hashes;

/// How to run a handoff.
pub struct Options<'a> {
    /// Staging directory from `tsync rewrite`. Expected hashes come from here.
    pub staging: PathBuf,
    /// Destination client. Torrents start here, never before the source stops.
    pub dest: &'a dyn Client,
    /// Source client. When present, torrents are stopped here first.
    pub source: Option<&'a dyn Client>,
    /// Start dest after you have stopped the source yourself. Required when
    /// `--source-url` is omitted and dest should announce.
    pub confirm: bool,
    /// Classify only; do not stop or start anything.
    pub dry_run: bool,
    /// Handoff only the smallest N staged torrents.
    pub max_torrents: Option<usize>,
}

/// One candidate after it is stopped/started (or fails).
#[derive(Clone, Debug)]
pub struct Tick {
    /// 1-based count of candidates processed so far.
    pub done: usize,
    /// Candidates this run will start (or try to).
    pub total: usize,
    /// Infohash just processed.
    pub id: String,
    /// True when dest `start` was called for this hash.
    pub started: bool,
}

/// Why handoff could not start.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Staging or dest listing failed before any torrent was touched.
    #[error(transparent)]
    Verify(#[from] tsync_verify::Error),
    /// A `WebAPI` call failed before per-torrent work began.
    #[error(transparent)]
    Api(#[from] tsync_qbt::Error),
}

/// Outcome of one handoff pass.
#[derive(Clone, Debug)]
pub struct Report {
    /// Dest is complete and stopped; not started this run.
    pub ready: usize,
    /// Dest `start` was called after the source was silent.
    pub started: usize,
    /// Dest was already seeding and the source was not.
    pub already: usize,
    /// Source `stop` was called.
    pub source_stopped: usize,
    /// Per-torrent failures. Dest was not started for these.
    pub failed: Vec<Failure>,
    /// True when nothing was written to either client.
    pub dry_run: bool,
    /// True when dest was not started because this was a report-only pass.
    pub report_only: bool,
}

/// One torrent that was not handed off.
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
            String::from("tsync handoff — dry run\n\n")
        } else {
            String::from("tsync handoff\n\n")
        };
        let _ = writeln!(out, "  ready           {:>5}", self.ready);
        let _ = writeln!(out, "  started         {:>5}", self.started);
        let _ = writeln!(out, "  already         {:>5}", self.already);
        let _ = writeln!(out, "  source stopped  {:>5}", self.source_stopped);
        let _ = writeln!(out, "  failed          {:>5}", self.failed.len());
        if !self.failed.is_empty() {
            let _ = writeln!(out);
            for item in &self.failed {
                let _ = writeln!(out, "  FAIL  {}  {}", short_id(&item.id), item.reason);
            }
        }
        if self.report_only && self.ready > 0 {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "  dest is ready and still stopped. Stop those hashes on the"
            );
            let _ = writeln!(
                out,
                "  source, then re-run with --confirm — or pass --source-url"
            );
            let _ = writeln!(out, "  to stop the source automatically.");
        }
        out.push('\n');
        out
    }

    /// True when every expected torrent is ready, started, or already handed.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

enum Class {
    Candidate,
    Already,
    Fail(String),
}

/// Run the handoff sequence for staged hashes.
///
/// # Errors
///
/// Returns [`Error`] when staging cannot be read or a client cannot be listed
/// before any stop/start.
pub fn run(opts: &Options<'_>) -> Result<Report, Error> {
    run_with_progress(opts, |_| {})
}

/// Same as [`run`], calling `progress` after each candidate so a CLI can
/// show that the command is still working.
///
/// # Errors
///
/// Same as [`run`].
pub fn run_with_progress(
    opts: &Options<'_>,
    mut progress: impl FnMut(Tick),
) -> Result<Report, Error> {
    let expected = expected_hashes(&opts.staging, opts.max_torrents)?;
    let dest_map = index(&opts.dest.list()?);
    let source_map = match opts.source {
        Some(source) => Some(index(&source.list()?)),
        None => None,
    };

    let mut ready = 0;
    let mut already = 0;
    let mut failed = Vec::new();
    let mut candidates = Vec::new();

    for hash in &expected {
        match classify(
            dest_map.get(hash),
            source_map.as_ref().and_then(|m| m.get(hash)),
            opts,
        ) {
            Class::Candidate => {
                ready += 1;
                candidates.push(hash.clone());
            }
            Class::Already => already += 1,
            Class::Fail(reason) => failed.push(Failure {
                id: hash.clone(),
                reason,
            }),
        }
    }

    let would_start = opts.source.is_some() || opts.confirm;
    if opts.dry_run {
        return Ok(Report {
            ready,
            started: if would_start { candidates.len() } else { 0 },
            already,
            source_stopped: 0,
            failed,
            dry_run: true,
            report_only: !would_start,
        });
    }
    if !would_start {
        return Ok(Report {
            ready,
            started: 0,
            already,
            source_stopped: 0,
            failed,
            dry_run: false,
            report_only: true,
        });
    }

    let applying_breaker = !candidates.is_empty();
    let previous = if applying_breaker {
        let previous = opts.dest.download_limit()?;
        opts.dest.set_download_limit(CIRCUIT_BREAKER_BPS)?;
        Some(previous)
    } else {
        None
    };
    let _guard = previous.map(|previous| LimitGuard {
        dest: opts.dest,
        previous,
    });

    let mut started = 0;
    let mut source_stopped = 0;
    let total = candidates.len();
    for (index, hash) in candidates.iter().enumerate() {
        let prior = source_map.as_ref().and_then(|map| map.get(hash));
        let mut did_start = false;
        match handoff_one(opts.source, opts.dest, hash, prior) {
            Ok(did_stop) => {
                started += 1;
                did_start = true;
                if did_stop {
                    source_stopped += 1;
                }
            }
            Err(reason) => failed.push(Failure {
                id: hash.clone(),
                reason,
            }),
        }
        progress(Tick {
            done: index + 1,
            total,
            id: hash.clone(),
            started: did_start,
        });
    }

    Ok(Report {
        ready,
        started,
        already,
        source_stopped,
        failed,
        dry_run: false,
        report_only: false,
    })
}

fn classify(dest: Option<&Torrent>, source: Option<&Torrent>, opts: &Options<'_>) -> Class {
    let Some(dest) = dest else {
        return Class::Fail("missing on destination; import and verify first".into());
    };
    if is_checking(&dest.state) {
        return Class::Fail(format!("still checking ({})", dest.state));
    }
    if is_downloading(&dest.state) {
        return Class::Fail(format!(
            "downloading ({}); dest must stay paused until handoff",
            dest.state
        ));
    }
    if !is_piece_complete(dest) {
        return Class::Fail(format!(
            "incomplete (progress {:.3}, {} bytes left)",
            dest.progress, dest.amount_left
        ));
    }
    if is_seeding(&dest.state) {
        if source.is_some_and(|src| is_seeding(&src.state)) {
            return Class::Fail("both clients are seeding this hash; stop the source first".into());
        }
        if opts.source.is_none() && !opts.confirm {
            return Class::Fail(
                "dest is seeding; pass --source-url to prove the source is stopped".into(),
            );
        }
        return Class::Already;
    }
    if !is_stopped(&dest.state) {
        return Class::Fail(format!("unexpected dest state {}", dest.state));
    }
    Class::Candidate
}

fn handoff_one(
    source: Option<&dyn Client>,
    dest: &dyn Client,
    hash: &str,
    prior: Option<&Torrent>,
) -> Result<bool, String> {
    let mut did_stop = false;
    if let Some(source) = source {
        let mut state = prior.map(|torrent| torrent.state.clone());
        if state.as_deref().is_some_and(|s| !is_stopped(s)) {
            source.stop(hash).map_err(|error| error.to_string())?;
            did_stop = true;
            // 5.x can still report stalledUP for a beat after torrents/stop.
            thread::sleep(Duration::from_millis(250));
            state = source_state(source, hash)?;
        }
        if state.as_deref().is_some_and(is_seeding) {
            return Err(format!(
                "source still seeding after stop ({}); dest was not started",
                state.unwrap_or_default()
            ));
        }
    }
    dest.start(hash).map_err(|error| error.to_string())?;
    Ok(did_stop)
}

fn source_state(source: &dyn Client, hash: &str) -> Result<Option<String>, String> {
    let listed = source.list().map_err(|error| error.to_string())?;
    Ok(find_hash(&listed, hash).map(|torrent| torrent.state.clone()))
}

fn find_hash<'a>(torrents: &'a [Torrent], hash: &str) -> Option<&'a Torrent> {
    torrents
        .iter()
        .find(|torrent| torrent.hash.eq_ignore_ascii_case(hash))
}

fn index(torrents: &[Torrent]) -> BTreeMap<String, Torrent> {
    torrents
        .iter()
        .map(|torrent| (torrent.hash.to_ascii_lowercase(), torrent.clone()))
        .collect()
}

fn short_id(id: &str) -> String {
    if id.len() > 12 {
        format!("{}…", &id[..12])
    } else {
        id.to_owned()
    }
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
