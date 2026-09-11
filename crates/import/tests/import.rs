//! Import against generated staging and fake clients. Never a real `WebUI`.

use std::sync::Mutex;

use tsync_fixtures::Builder;
use tsync_import::{Options, run, run_with_progress};
use tsync_plan::DEFAULT_BUDGET;
use tsync_qbt::{Client, Error as QbtError, Torrent};
use tsync_rewrite::{Options as RewriteOptions, run as rewrite};

struct Fake {
    list: Vec<Torrent>,
    added: Mutex<Vec<(String, String)>>,
    stopped: Mutex<Vec<String>>,
    rechecked: Mutex<Vec<String>>,
    limits: Mutex<Vec<i64>>,
    current_limit: Mutex<i64>,
    fail_add: bool,
}

impl Fake {
    fn empty() -> Self {
        Self {
            list: Vec::new(),
            added: Mutex::new(Vec::new()),
            stopped: Mutex::new(Vec::new()),
            rechecked: Mutex::new(Vec::new()),
            limits: Mutex::new(Vec::new()),
            current_limit: Mutex::new(0),
            fail_add: false,
        }
    }

    fn seeding(hash: &str) -> Self {
        Self {
            list: vec![Torrent {
                hash: hash.to_owned(),
                name: "red".into(),
                state: "stalledUP".into(),
                progress: 1.0,
                amount_left: 0,
                save_path: "/srv/music".into(),
                size: 0,
                completed: 0,
            }],
            added: Mutex::new(Vec::new()),
            stopped: Mutex::new(Vec::new()),
            rechecked: Mutex::new(Vec::new()),
            limits: Mutex::new(Vec::new()),
            current_limit: Mutex::new(0),
            fail_add: false,
        }
    }
}

impl Client for Fake {
    fn list(&self) -> Result<Vec<Torrent>, QbtError> {
        Ok(self.list.clone())
    }

    fn add_paused(&self, _torrent: &[u8], filename: &str, save_path: &str) -> Result<(), QbtError> {
        if self.fail_add {
            return Err(QbtError::Unexpected("Fails.".into()));
        }
        self.added
            .lock()
            .expect("added")
            .push((filename.to_owned(), save_path.to_owned()));
        Ok(())
    }

    fn stop(&self, hash: &str) -> Result<(), QbtError> {
        self.stopped.lock().expect("stopped").push(hash.to_owned());
        Ok(())
    }

    fn start(&self, _hash: &str) -> Result<(), QbtError> {
        panic!("import must never start dest");
    }

    fn recheck(&self, hash: &str) -> Result<(), QbtError> {
        self.rechecked
            .lock()
            .expect("rechecked")
            .push(hash.to_owned());
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

fn staged() -> (
    tempfile::TempDir,
    tsync_fixtures::Library,
    tempfile::TempDir,
) {
    let root = tempfile::tempdir().expect("root");
    let library = Builder::new()
        .complete("red", "/srv/music", &[("a.flac", 1024)])
        .complete("blue", "/srv/music", &[("b.flac", 2048)])
        .claims_complete_but_missing("ghost", "/srv/music", &[("g.flac", 4096)], 1)
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

#[test]
fn refuses_without_a_source_unless_explicitly_allowed() {
    let (_root, _library, staging) = staged();
    let dest = Fake::empty();
    let error = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: None,
        allow_unverified_source: false,
        dry_run: false,
        max_torrents: None,
    })
    .expect_err("source required");
    assert!(error.to_string().contains("source-url"));
}

#[test]
fn refuses_when_dest_and_source_both_seed() {
    let (_root, library, staging) = staged();
    let hash = library.torrents[0].infohash.clone();
    let dest = Fake::seeding(&hash);
    let source = Fake::seeding(&hash);

    let error = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        allow_unverified_source: false,
        dry_run: false,
        max_torrents: None,
    })
    .expect_err("dual seed");
    assert!(error.to_string().contains("already seeding"));
    assert!(dest.added.lock().expect("added").is_empty());
}

#[test]
fn source_may_still_seed_when_dest_is_empty() {
    let (_root, library, staging) = staged();
    let hash = library.torrents[0].infohash.clone();
    let dest = Fake::empty();
    let source = Fake::seeding(&hash);

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        allow_unverified_source: false,
        dry_run: false,
        max_torrents: None,
    })
    .expect("import");
    assert_eq!(report.imported, 2);
    assert_eq!(dest.added.lock().expect("added").len(), 2);
}

#[test]
fn adds_paused_rechecks_and_restores_the_download_limit() {
    let (_root, _library, staging) = staged();
    let dest = Fake::empty();
    let source = Fake::empty();

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        allow_unverified_source: false,
        dry_run: false,
        max_torrents: None,
    })
    .expect("import");

    assert_eq!(report.imported, 2);
    assert_eq!(report.already_present, 0);
    assert!(report.failed.is_empty());

    let added = dest.added.lock().expect("added");
    assert_eq!(added.len(), 2);
    assert!(added.iter().all(|(_, path)| path == "/data"));

    let stopped = dest.stopped.lock().expect("stopped");
    assert_eq!(stopped.len(), 4, "stop before and after recheck");
    assert_eq!(dest.rechecked.lock().expect("rechecked").len(), 2);
    assert_eq!(&stopped[0], &stopped[1]);

    let limits = dest.limits.lock().expect("limits");
    assert_eq!(limits[0], 1, "circuit breaker first");
    assert_eq!(*limits.last().expect("restored"), 0);
}

#[test]
fn skips_hashes_already_on_the_destination() {
    let (_root, library, staging) = staged();
    let hash = library.torrents[0].infohash.clone();
    let dest = Fake::seeding(&hash);
    let source = Fake::empty();

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        allow_unverified_source: false,
        dry_run: false,
        max_torrents: None,
    })
    .expect("import");

    assert_eq!(report.already_present, 1);
    assert_eq!(report.imported, 1);
    assert_eq!(dest.added.lock().expect("added").len(), 1);
    assert_eq!(dest.rechecked.lock().expect("rechecked").len(), 2);
}

#[test]
fn dry_run_does_not_add_or_change_limits() {
    let (_root, _library, staging) = staged();
    let dest = Fake::empty();
    let source = Fake::empty();

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        allow_unverified_source: false,
        dry_run: true,
        max_torrents: None,
    })
    .expect("dry run");

    assert!(report.dry_run);
    assert_eq!(report.imported, 2);
    assert!(dest.added.lock().expect("added").is_empty());
    assert!(dest.limits.lock().expect("limits").is_empty());
}

#[test]
fn restores_the_limit_when_an_add_fails() {
    let (_root, _library, staging) = staged();
    let dest = Fake {
        fail_add: true,
        ..Fake::empty()
    };
    let source = Fake::empty();

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        allow_unverified_source: false,
        dry_run: false,
        max_torrents: None,
    })
    .expect("report");

    assert_eq!(report.failed.len(), 2);
    let limits = dest.limits.lock().expect("limits");
    assert_eq!(*limits.last().expect("restored"), 0);
}

#[test]
fn max_torrents_imports_only_the_smallest() {
    let (_root, _library, staging) = staged();
    let dest = Fake::empty();
    let source = Fake::empty();

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        source: Some(&source),
        allow_unverified_source: false,
        dry_run: false,
        max_torrents: Some(1),
    })
    .expect("import");

    assert_eq!(report.imported, 1);
    assert_eq!(dest.added.lock().expect("added").len(), 1);
}

#[test]
fn progress_ticks_once_per_staged_item() {
    let (_root, _library, staging) = staged();
    let dest = Fake::empty();
    let source = Fake::empty();
    let ticks = Mutex::new(Vec::new());

    run_with_progress(
        &Options {
            staging: staging.path().to_path_buf(),
            dest: &dest,
            source: Some(&source),
            allow_unverified_source: false,
            dry_run: false,
            max_torrents: None,
        },
        |tick| {
            ticks.lock().expect("ticks").push((tick.done, tick.total));
        },
    )
    .expect("import");

    assert_eq!(ticks.lock().expect("ticks").clone(), vec![(1, 2), (2, 2)]);
}
