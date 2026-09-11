//! Migrate against generated libraries and fake clients.

use std::sync::Mutex;

use tsync_fixtures::Builder;
use tsync_migrate::{Options, Step, run};
use tsync_plan::DEFAULT_BUDGET;
use tsync_qbt::{Client, Error as QbtError, Torrent};
use tsync_rewrite::{Options as RewriteOptions, run as rewrite};

struct Fake {
    torrents: Mutex<Vec<Torrent>>,
    added: Mutex<usize>,
    started: Mutex<Vec<String>>,
    stopped: Mutex<Vec<String>>,
    limits: Mutex<Vec<i64>>,
    current_limit: Mutex<i64>,
}

impl Fake {
    fn empty() -> Self {
        Self::with(Vec::new())
    }

    fn with(torrents: Vec<Torrent>) -> Self {
        Self {
            torrents: Mutex::new(torrents),
            added: Mutex::new(0),
            started: Mutex::new(Vec::new()),
            stopped: Mutex::new(Vec::new()),
            limits: Mutex::new(Vec::new()),
            current_limit: Mutex::new(0),
        }
    }
}

impl Client for Fake {
    fn list(&self) -> Result<Vec<Torrent>, QbtError> {
        Ok(self.torrents.lock().expect("torrents").clone())
    }

    fn add_paused(&self, _torrent: &[u8], filename: &str, save_path: &str) -> Result<(), QbtError> {
        *self.added.lock().expect("added") += 1;
        let hash = filename
            .strip_suffix(".torrent")
            .unwrap_or(filename)
            .to_owned();
        self.torrents.lock().expect("torrents").push(Torrent {
            hash,
            name: "x".into(),
            state: "stoppedUP".into(),
            progress: 1.0,
            amount_left: 0,
            save_path: save_path.to_owned(),
            size: 0,
            completed: 0,
        });
        Ok(())
    }

    fn stop(&self, hash: &str) -> Result<(), QbtError> {
        self.stopped.lock().expect("stopped").push(hash.to_owned());
        for torrent in self.torrents.lock().expect("torrents").iter_mut() {
            if torrent.hash.eq_ignore_ascii_case(hash) {
                torrent.state = "stoppedUP".into();
            }
        }
        Ok(())
    }

    fn start(&self, hash: &str) -> Result<(), QbtError> {
        self.started.lock().expect("started").push(hash.to_owned());
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

fn torrent(hash: &str, state: &str, progress: f64, amount_left: u64) -> Torrent {
    Torrent {
        hash: hash.to_owned(),
        name: "x".into(),
        state: state.into(),
        progress,
        amount_left,
        save_path: "/data".into(),
        size: 0,
        completed: 0,
    }
}

fn library() -> (
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
    (root, library, staging)
}

fn seeding(library: &tsync_fixtures::Library) -> Fake {
    Fake::with(
        library
            .torrents
            .iter()
            .map(|item| torrent(&item.infohash, "stalledUP", 1.0, 0))
            .collect(),
    )
}

fn ready(library: &tsync_fixtures::Library) -> Fake {
    Fake::with(
        library
            .torrents
            .iter()
            .map(|item| torrent(&item.infohash, "stoppedUP", 1.0, 0))
            .collect(),
    )
}

#[test]
fn stops_at_verify_and_does_not_start_dest() {
    let (_root, library, staging) = library();
    let bytes = tempfile::tempdir().expect("bytes");
    let dest = Fake::empty();
    let source = seeding(&library);

    let report = run(&Options {
        bt_backup: library.bt_backup.clone(),
        staging: staging.path().to_path_buf(),
        dest: "/data".into(),
        rsync_to: Some(bytes.path().to_string_lossy().into_owned()),
        data_root: Some(library.data_root.clone()),
        budget: DEFAULT_BUDGET,
        dest_client: &dest,
        source_client: Some(&source),
        allow_unverified_source: false,
        handoff: false,
        confirm: false,
        from_step: Step::Rewrite,
        dry_run: false,
        max_torrents: None,
    })
    .expect("migrate");

    assert!(report.is_complete());
    assert_eq!(report.stopped_at, "verify");
    assert_eq!(report.transferred, 2);
    assert_eq!(report.imported, 2);
    assert_eq!(report.ready, 2);
    assert_eq!(report.started, 0);
    assert!(dest.started.lock().expect("started").is_empty());
    assert!(bytes.path().join("red").join("a.flac").is_file());
}

fn write_staging(library: &tsync_fixtures::Library, staging: &std::path::Path) {
    rewrite(&RewriteOptions {
        bt_backup: library.bt_backup.clone(),
        staging: staging.to_path_buf(),
        dest: "/data".to_owned(),
        budget: DEFAULT_BUDGET,
        data_root: Some(library.data_root.clone()),
    })
    .expect("rewrite");
}

#[test]
fn handoff_stops_source_then_starts_dest() {
    let (_root, library, staging) = library();
    write_staging(&library, staging.path());
    let dest = ready(&library);
    let source = seeding(&library);

    let report = run(&Options {
        bt_backup: library.bt_backup.clone(),
        staging: staging.path().to_path_buf(),
        dest: "/data".into(),
        rsync_to: None,
        data_root: Some(library.data_root.clone()),
        budget: DEFAULT_BUDGET,
        dest_client: &dest,
        source_client: Some(&source),
        allow_unverified_source: false,
        handoff: true,
        confirm: false,
        from_step: Step::Handoff,
        dry_run: false,
        max_torrents: None,
    })
    .expect("migrate");

    assert_eq!(report.stopped_at, "handoff");
    assert_eq!(report.started, 2);
    assert_eq!(source.stopped.lock().expect("stopped").len(), 2);
    assert_eq!(dest.started.lock().expect("started").len(), 2);
}

#[test]
fn incomplete_verify_does_not_handoff() {
    let (_root, library, staging) = library();
    write_staging(&library, staging.path());
    let dest = Fake::with(
        library
            .torrents
            .iter()
            .map(|item| torrent(&item.infohash, "stoppedUP", 0.5, 512))
            .collect(),
    );
    let source = seeding(&library);

    let report = run(&Options {
        bt_backup: library.bt_backup.clone(),
        staging: staging.path().to_path_buf(),
        dest: "/data".into(),
        rsync_to: None,
        data_root: Some(library.data_root.clone()),
        budget: DEFAULT_BUDGET,
        dest_client: &dest,
        source_client: Some(&source),
        allow_unverified_source: false,
        handoff: true,
        confirm: false,
        from_step: Step::Verify,
        dry_run: false,
        max_torrents: None,
    })
    .expect("migrate");

    assert!(report.halted);
    assert_eq!(report.stopped_at, "verify");
    assert!(dest.started.lock().expect("started").is_empty());
    assert!(source.stopped.lock().expect("stopped").is_empty());
}

#[test]
fn dry_run_does_not_add_or_start() {
    let (_root, library, staging) = library();
    let bytes = tempfile::tempdir().expect("bytes");
    let dest = Fake::empty();
    let source = Fake::empty();

    let report = run(&Options {
        bt_backup: library.bt_backup.clone(),
        staging: staging.path().to_path_buf(),
        dest: "/data".into(),
        rsync_to: Some(bytes.path().to_string_lossy().into_owned()),
        data_root: Some(library.data_root.clone()),
        budget: DEFAULT_BUDGET,
        dest_client: &dest,
        source_client: Some(&source),
        allow_unverified_source: false,
        handoff: true,
        confirm: false,
        from_step: Step::Rewrite,
        dry_run: true,
        max_torrents: None,
    })
    .expect("migrate");

    assert!(report.dry_run);
    assert_eq!(*dest.added.lock().expect("added"), 0);
    assert!(dest.started.lock().expect("started").is_empty());
    assert!(
        std::fs::read_dir(bytes.path())
            .expect("bytes")
            .next()
            .is_none()
    );
}
