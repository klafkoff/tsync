//! Handoff against generated staging and fake clients. Never a real `WebUI`.

use std::sync::{Arc, Mutex};

use tsync_fixtures::Builder;
use tsync_handoff::{Options, run};
use tsync_plan::DEFAULT_BUDGET;
use tsync_qbt::{Client, Error as QbtError, Torrent};
use tsync_rewrite::{Options as RewriteOptions, run as rewrite};

struct Fake {
    torrents: Mutex<Vec<Torrent>>,
    log: Mutex<Vec<String>>,
    shared: Option<Arc<Mutex<Vec<String>>>>,
    role: &'static str,
    limits: Mutex<Vec<i64>>,
    current_limit: Mutex<i64>,
    /// `stop` succeeds but leaves the torrent seeding.
    ignore_stop: bool,
}

impl Fake {
    fn with(torrents: Vec<Torrent>) -> Self {
        Self {
            torrents: Mutex::new(torrents),
            log: Mutex::new(Vec::new()),
            shared: None,
            role: "client",
            limits: Mutex::new(Vec::new()),
            current_limit: Mutex::new(0),
            ignore_stop: false,
        }
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().expect("log").clone()
    }

    fn record(&self, op: &str, hash: &str) {
        let line = format!("{op}:{hash}");
        self.log.lock().expect("log").push(line);
        if let Some(shared) = &self.shared {
            shared
                .lock()
                .expect("shared")
                .push(format!("{}-{op}:{hash}", self.role));
        }
    }
}

impl Client for Fake {
    fn list(&self) -> Result<Vec<Torrent>, QbtError> {
        Ok(self.torrents.lock().expect("torrents").clone())
    }

    fn add_paused(
        &self,
        _torrent: &[u8],
        _filename: &str,
        _save_path: &str,
    ) -> Result<(), QbtError> {
        Ok(())
    }

    fn stop(&self, hash: &str) -> Result<(), QbtError> {
        self.record("stop", hash);
        if self.ignore_stop {
            return Ok(());
        }
        for torrent in self.torrents.lock().expect("torrents").iter_mut() {
            if torrent.hash.eq_ignore_ascii_case(hash) {
                torrent.state = "stoppedUP".into();
            }
        }
        Ok(())
    }

    fn start(&self, hash: &str) -> Result<(), QbtError> {
        self.record("start", hash);
        for torrent in self.torrents.lock().expect("torrents").iter_mut() {
            if torrent.hash.eq_ignore_ascii_case(hash) {
                torrent.state = "stalledUP".into();
            }
        }
        Ok(())
    }

    fn recheck(&self, _hash: &str) -> Result<(), QbtError> {
        Ok(())
    }

    fn download_limit(&self) -> Result<i64, QbtError> {
        Ok(*self.current_limit.lock().expect("limit"))
    }

    fn set_download_limit(&self, bytes_per_sec: i64) -> Result<(), QbtError> {
        self.limits.lock().expect("limits").push(bytes_per_sec);
        *self.current_limit.lock().expect("limit") = bytes_per_sec;
        Ok(())
    }
}

fn torrent(hash: &str, state: &str) -> Torrent {
    Torrent {
        hash: hash.to_owned(),
        name: "x".into(),
        state: state.into(),
        progress: 1.0,
        amount_left: 0,
        save_path: "/data".into(),
    }
}

fn incomplete(hash: &str) -> Torrent {
    Torrent {
        hash: hash.to_owned(),
        name: "x".into(),
        state: "stoppedUP".into(),
        progress: 0.5,
        amount_left: 512,
        save_path: "/data".into(),
    }
}

fn staged() -> (
    tempfile::TempDir,
    tsync_fixtures::Library,
    tempfile::TempDir,
) {
    let root = tempfile::tempdir().expect("root");
    let library = Builder::new()
        .complete("red", "/srv/music", &[("a.flac", 1024)])
        .complete("blue", "/srv/music", &[("b.flac", 2048)])
        .materialize(root.path())
        .expect("materialize");
    let staging = tempfile::tempdir().expect("staging");
    rewrite(&RewriteOptions {
        bt_backup: library.bt_backup.clone(),
        staging: staging.path().to_path_buf(),
        dest: "/data".to_owned(),
        budget: DEFAULT_BUDGET,
        data_root: Some(library.data_root.clone()),
    })
    .expect("rewrite");
    (root, library, staging)
}

fn hashes(library: &tsync_fixtures::Library) -> (String, String) {
    (
        library.torrents[0].infohash.clone(),
        library.torrents[1].infohash.clone(),
    )
}

fn dest_ready(library: &tsync_fixtures::Library) -> Fake {
    Fake::with(
        library
            .torrents
            .iter()
            .map(|item| torrent(&item.infohash, "stoppedUP"))
            .collect(),
    )
}

fn source_seeding(library: &tsync_fixtures::Library) -> Fake {
    Fake::with(
        library
            .torrents
            .iter()
            .map(|item| torrent(&item.infohash, "stalledUP"))
            .collect(),
    )
}

#[test]
fn report_only_does_not_stop_or_start() {
    let (_root, library, staging) = staged();
    let dest = dest_ready(&library);
    let source = source_seeding(&library);

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: None,
        confirm: false,
        dry_run: false,
        max_torrents: None,
    })
    .expect("handoff");

    assert!(report.report_only);
    assert_eq!(report.ready, 2);
    assert_eq!(report.started, 0);
    assert!(report.render().contains("--confirm"));
    assert!(dest.log().is_empty());
    assert!(source.log().is_empty());
}

#[test]
fn auto_stops_source_before_starting_dest() {
    let (_root, library, staging) = staged();
    let (red, blue) = hashes(&library);
    let shared = Arc::new(Mutex::new(Vec::new()));
    let mut dest = dest_ready(&library);
    dest.shared = Some(Arc::clone(&shared));
    dest.role = "dest";
    let mut source = source_seeding(&library);
    source.shared = Some(Arc::clone(&shared));
    source.role = "source";

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        confirm: false,
        dry_run: false,
        max_torrents: None,
    })
    .expect("handoff");

    assert_eq!(report.started, 2);
    assert_eq!(report.source_stopped, 2);
    assert!(report.failed.is_empty());
    assert!(report.is_complete());

    let timeline = shared.lock().expect("shared").clone();
    for hash in [&red, &blue] {
        let stop = timeline
            .iter()
            .position(|line| line == &format!("source-stop:{hash}"))
            .expect("source stopped first");
        let start = timeline
            .iter()
            .position(|line| line == &format!("dest-start:{hash}"))
            .expect("dest started after");
        assert!(
            stop < start,
            "source must stop before dest starts: {timeline:?}"
        );
    }

    let limits = dest.limits.lock().expect("limits");
    assert_eq!(limits[0], 1, "circuit breaker first");
    assert_eq!(*limits.last().expect("restored"), 0);
}

#[test]
fn never_starts_dest_if_source_still_seeds_after_stop() {
    let (_root, library, staging) = staged();
    let dest = dest_ready(&library);
    let mut source = source_seeding(&library);
    source.ignore_stop = true;

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        confirm: false,
        dry_run: false,
        max_torrents: None,
    })
    .expect("handoff");

    assert_eq!(report.started, 0);
    assert_eq!(report.failed.len(), 2);
    assert!(
        report.failed[0]
            .reason
            .contains("source still seeding after stop")
    );
    assert!(dest.log().is_empty());
}

#[test]
fn dest_incomplete_does_not_stop_source_or_start_dest() {
    let (_root, library, staging) = staged();
    let (red, blue) = hashes(&library);
    let dest = Fake::with(vec![incomplete(&red), torrent(&blue, "stoppedUP")]);
    let source = source_seeding(&library);

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        confirm: false,
        dry_run: false,
        max_torrents: None,
    })
    .expect("handoff");

    assert_eq!(report.started, 1);
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].reason.contains("incomplete"));
    assert!(
        !source
            .log()
            .iter()
            .any(|line| line == &format!("stop:{red}"))
    );
    assert!(
        !dest
            .log()
            .iter()
            .any(|line| line == &format!("start:{red}"))
    );
    assert!(source.log().contains(&format!("stop:{blue}")));
    assert!(dest.log().contains(&format!("start:{blue}")));
}

#[test]
fn both_seeding_is_refused_and_neither_client_is_touched() {
    let (_root, library, staging) = staged();
    let dest = Fake::with(
        library
            .torrents
            .iter()
            .map(|item| torrent(&item.infohash, "stalledUP"))
            .collect(),
    );
    let source = source_seeding(&library);

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        confirm: false,
        dry_run: false,
        max_torrents: None,
    })
    .expect("handoff");

    assert_eq!(report.failed.len(), 2);
    assert!(report.failed[0].reason.contains("both clients are seeding"));
    assert!(dest.log().is_empty());
    assert!(source.log().is_empty());
}

#[test]
fn dest_already_seeding_and_source_stopped_is_already() {
    let (_root, library, staging) = staged();
    let dest = Fake::with(
        library
            .torrents
            .iter()
            .map(|item| torrent(&item.infohash, "stalledUP"))
            .collect(),
    );
    let source = Fake::with(
        library
            .torrents
            .iter()
            .map(|item| torrent(&item.infohash, "stoppedUP"))
            .collect(),
    );

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        confirm: false,
        dry_run: false,
        max_torrents: None,
    })
    .expect("handoff");

    assert_eq!(report.already, 2);
    assert_eq!(report.started, 0);
    assert!(report.is_complete());
    assert!(dest.log().is_empty());
    assert!(source.log().is_empty());
}

#[test]
fn confirm_without_source_starts_ready_dest() {
    let (_root, library, staging) = staged();
    let dest = dest_ready(&library);

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: None,
        confirm: true,
        dry_run: false,
        max_torrents: None,
    })
    .expect("handoff");

    assert_eq!(report.started, 2);
    assert_eq!(dest.log().len(), 2);
    assert!(dest.log().iter().all(|line| line.starts_with("start:")));
}

#[test]
fn dry_run_with_source_does_not_touch_clients() {
    let (_root, library, staging) = staged();
    let dest = dest_ready(&library);
    let source = source_seeding(&library);

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        confirm: false,
        dry_run: true,
        max_torrents: None,
    })
    .expect("handoff");

    assert!(report.dry_run);
    assert_eq!(report.started, 2);
    assert!(dest.log().is_empty());
    assert!(source.log().is_empty());
    assert!(dest.limits.lock().expect("limits").is_empty());
}

#[test]
fn retry_starts_dest_when_source_is_already_stopped() {
    let (_root, library, staging) = staged();
    let dest = dest_ready(&library);
    let source = Fake::with(
        library
            .torrents
            .iter()
            .map(|item| torrent(&item.infohash, "stoppedUP"))
            .collect(),
    );

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        confirm: false,
        dry_run: false,
        max_torrents: None,
    })
    .expect("handoff");

    assert_eq!(report.started, 2);
    assert_eq!(report.source_stopped, 0);
    assert!(source.log().is_empty());
    assert_eq!(dest.log().len(), 2);
}

#[test]
fn max_torrents_hands_off_only_the_smallest() {
    let (_root, library, staging) = staged();
    let (red, blue) = hashes(&library);
    let dest = dest_ready(&library);
    let source = source_seeding(&library);

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        confirm: false,
        dry_run: false,
        max_torrents: Some(1),
    })
    .expect("handoff");

    assert_eq!(report.started, 1);
    assert_eq!(source.log(), vec![format!("stop:{red}")]);
    assert_eq!(dest.log(), vec![format!("start:{red}")]);
    assert!(!source.log().iter().any(|line| line.contains(&blue)));
}
