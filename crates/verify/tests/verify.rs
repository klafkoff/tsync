//! Verify against generated staging and fake clients. Never a real `WebUI`.

use tsync_fixtures::Builder;
use tsync_plan::DEFAULT_BUDGET;
use tsync_qbt::{Client, Error as QbtError, Torrent};
use tsync_rewrite::{Options as RewriteOptions, run as rewrite};
use tsync_verify::{Options, run};

struct Fake {
    list: Vec<Torrent>,
}

impl Fake {
    fn with(torrents: Vec<Torrent>) -> Self {
        Self { list: torrents }
    }
}

impl Client for Fake {
    fn list(&self) -> Result<Vec<Torrent>, QbtError> {
        Ok(self.list.clone())
    }

    fn add_paused(&self, _torrent: &[u8], _filename: &str, _save_path: &str) -> Result<(), QbtError> {
        Ok(())
    }

    fn stop(&self, _hash: &str) -> Result<(), QbtError> {
        Ok(())
    }

    fn recheck(&self, _hash: &str) -> Result<(), QbtError> {
        Ok(())
    }

    fn download_limit(&self) -> Result<i64, QbtError> {
        Ok(0)
    }

    fn set_download_limit(&self, _bytes_per_sec: i64) -> Result<(), QbtError> {
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
    }
}

fn staged() -> (tempfile::TempDir, tsync_fixtures::Library, tempfile::TempDir) {
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

#[test]
fn ready_when_stopped_and_complete() {
    let (_root, library, staging) = staged();
    let dest = Fake::with(
        library
            .torrents
            .iter()
            .map(|item| torrent(&item.infohash, "stoppedUP", 1.0, 0))
            .collect(),
    );
    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        allow_seeding: false,
        max_torrents: None,
    })
    .expect("verify");
    assert!(report.is_complete());
    assert_eq!(report.ready, 2);
    assert!(report.render().contains("ready"));
}

#[test]
fn fails_when_dest_is_still_seeding() {
    let (_root, library, staging) = staged();
    let dest = Fake::with(
        library
            .torrents
            .iter()
            .map(|item| torrent(&item.infohash, "stalledUP", 1.0, 0))
            .collect(),
    );
    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        allow_seeding: false,
        max_torrents: None,
    })
    .expect("verify");
    assert!(!report.is_complete());
    assert_eq!(report.failed.len(), 2);
    assert!(report.failed[0].reason.contains("seeding"));
}

#[test]
fn allow_seeding_accepts_a_complete_seed() {
    let (_root, library, staging) = staged();
    let dest = Fake::with(
        library
            .torrents
            .iter()
            .map(|item| torrent(&item.infohash, "stalledUP", 1.0, 0))
            .collect(),
    );
    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &dest,
        allow_seeding: true,
        max_torrents: None,
    })
    .expect("verify");
    assert!(report.is_complete());
}

#[test]
fn incomplete_checking_missing_and_downloading_each_fail() {
    let (_root, library, staging) = staged();
    let red = &library.torrents[0].infohash;
    let blue = &library.torrents[1].infohash;

    let incomplete = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &Fake::with(vec![
            torrent(red, "stoppedUP", 0.5, 512),
            torrent(blue, "stoppedUP", 1.0, 0),
        ]),
        allow_seeding: false,
        max_torrents: None,
    })
    .expect("incomplete");
    assert_eq!(incomplete.ready, 1);
    assert_eq!(incomplete.failed.len(), 1);
    assert!(incomplete.failed[0].reason.contains("incomplete"));

    let checking = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &Fake::with(vec![
            torrent(red, "checkingUP", 0.0, 1024),
            torrent(blue, "stoppedUP", 1.0, 0),
        ]),
        allow_seeding: false,
        max_torrents: None,
    })
    .expect("checking");
    assert_eq!(checking.checking, 1);
    assert_eq!(checking.ready, 1);
    assert!(!checking.is_complete());

    let missing = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &Fake::with(vec![torrent(blue, "stoppedUP", 1.0, 0)]),
        allow_seeding: false,
        max_torrents: None,
    })
    .expect("missing");
    assert_eq!(missing.missing, 1);
    assert_eq!(missing.ready, 1);

    let downloading = run(&Options {
        staging: staging.path().to_path_buf(),
        dest: &Fake::with(vec![
            torrent(red, "stalledDL", 0.2, 800),
            torrent(blue, "stoppedUP", 1.0, 0),
        ]),
        allow_seeding: false,
        max_torrents: None,
    })
    .expect("downloading");
    assert_eq!(downloading.failed.len(), 1);
    assert!(downloading.failed[0].reason.contains("downloading"));
}
