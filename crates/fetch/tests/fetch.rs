//! Fetch against generated staging and fake clients. Never a real `WebUI`.

use std::fs;
use std::sync::Mutex;

use tsync_fetch::{Options, PartialMode, run};
use tsync_fixtures::Builder;
use tsync_plan::DEFAULT_BUDGET;
use tsync_qbt::{Client, Error as QbtError, Torrent, TorrentFile};
use tsync_rewrite::{Options as RewriteOptions, run as rewrite};
use tsync_transfer::{Options as TransferOptions, run as transfer};

struct Fake {
    list: Vec<Torrent>,
    files: Vec<TorrentFile>,
    added: Mutex<usize>,
    started: Mutex<usize>,
    stopped: Mutex<usize>,
}

impl Fake {
    fn with(list: Vec<Torrent>) -> Self {
        Self {
            list,
            files: Vec::new(),
            added: Mutex::new(0),
            started: Mutex::new(0),
            stopped: Mutex::new(0),
        }
    }
}

impl Client for Fake {
    fn list(&self) -> Result<Vec<Torrent>, QbtError> {
        Ok(self.list.clone())
    }

    fn add_paused(
        &self,
        _torrent: &[u8],
        _filename: &str,
        _save_path: &str,
    ) -> Result<(), QbtError> {
        *self.added.lock().expect("added") += 1;
        Ok(())
    }

    fn stop(&self, _hash: &str) -> Result<(), QbtError> {
        *self.stopped.lock().expect("stopped") += 1;
        Ok(())
    }

    fn start(&self, _hash: &str) -> Result<(), QbtError> {
        *self.started.lock().expect("started") += 1;
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

    fn files(&self, _hash: &str) -> Result<Vec<TorrentFile>, QbtError> {
        Ok(self.files.clone())
    }
}

fn torrent(hash: &str, progress: f64, amount_left: u64) -> Torrent {
    Torrent {
        hash: hash.to_owned(),
        name: "x".into(),
        state: "stoppedUP".into(),
        progress,
        amount_left,
        save_path: "/data".into(),
        size: 0,
        completed: 0,
    }
}

fn staged() -> (
    tempfile::TempDir,
    tsync_fixtures::Library,
    tempfile::TempDir,
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
    let remote = tempfile::tempdir().expect("remote");
    transfer(&TransferOptions {
        bt_backup: library.bt_backup.clone(),
        dest: "/data".to_owned(),
        rsync_to: Some(remote.path().to_string_lossy().into_owned()),
        budget: DEFAULT_BUDGET,
        data_root: Some(library.data_root.clone()),
        max_bytes: None,
        max_torrents: None,
        dry_run: false,
    })
    .expect("transfer");
    (root, library, staging, remote)
}

fn complete_remote(library: &tsync_fixtures::Library) -> Fake {
    Fake::with(
        library
            .torrents
            .iter()
            .map(|item| torrent(&item.infohash, 1.0, 0))
            .collect(),
    )
}

#[test]
fn copies_complete_torrents_and_never_imports() {
    let (_root, library, staging, remote) = staged();
    let dest = tempfile::tempdir().expect("laptop");
    let remote_client = complete_remote(&library);

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        from: remote.path().to_string_lossy().into_owned(),
        to: dest.path().to_string_lossy().into_owned(),
        save_root: "/data".into(),
        remote: &remote_client,
        partial: PartialMode::Skip,
        dry_run: false,
        max_torrents: None,
    })
    .expect("fetch");

    assert_eq!(report.fetched, 2);
    assert_eq!(report.skipped, 0);
    assert!(dest.path().join("red").join("a.flac").is_file());
    assert!(dest.path().join("blue").join("b.flac").is_file());
    assert_eq!(*remote_client.added.lock().expect("added"), 0);
    assert_eq!(*remote_client.started.lock().expect("started"), 0);
    assert_eq!(*remote_client.stopped.lock().expect("stopped"), 0);
}

#[test]
fn skips_incomplete_torrents_by_default() {
    let (_root, library, staging, remote) = staged();
    let dest = tempfile::tempdir().expect("laptop");
    let red = &library.torrents[0].infohash;
    let blue = &library.torrents[1].infohash;
    let remote_client = Fake::with(vec![torrent(red, 0.5, 512), torrent(blue, 1.0, 0)]);

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        from: remote.path().to_string_lossy().into_owned(),
        to: dest.path().to_string_lossy().into_owned(),
        save_root: "/data".into(),
        remote: &remote_client,
        partial: PartialMode::Skip,
        dry_run: false,
        max_torrents: None,
    })
    .expect("fetch");

    assert_eq!(report.fetched, 1);
    assert_eq!(report.skipped, 1);
    assert!(!dest.path().join("red").exists());
    assert!(dest.path().join("blue").join("b.flac").is_file());
}

#[test]
fn partial_all_copies_incomplete_files() {
    let (_root, library, staging, remote) = staged();
    let dest = tempfile::tempdir().expect("laptop");
    let red = &library.torrents[0].infohash;
    let blue = &library.torrents[1].infohash;
    let remote_client = Fake::with(vec![torrent(red, 0.5, 512), torrent(blue, 1.0, 0)]);

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        from: remote.path().to_string_lossy().into_owned(),
        to: dest.path().to_string_lossy().into_owned(),
        save_root: "/data".into(),
        remote: &remote_client,
        partial: PartialMode::All,
        dry_run: false,
        max_torrents: None,
    })
    .expect("fetch");

    assert_eq!(report.fetched, 2);
    assert_eq!(report.skipped, 0);
    assert!(dest.path().join("red").join("a.flac").is_file());
}

#[test]
fn complete_files_copies_only_finished_members() {
    let root = tempfile::tempdir().expect("root");
    let library = Builder::new()
        .complete("duo", "/srv/music", &[("a.flac", 1024), ("c.flac", 512)])
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
    let remote = tempfile::tempdir().expect("remote");
    transfer(&TransferOptions {
        bt_backup: library.bt_backup.clone(),
        dest: "/data".to_owned(),
        rsync_to: Some(remote.path().to_string_lossy().into_owned()),
        budget: DEFAULT_BUDGET,
        data_root: Some(library.data_root.clone()),
        max_bytes: None,
        max_torrents: None,
        dry_run: false,
    })
    .expect("transfer");

    let hash = &library.torrents[0].infohash;
    let mut remote_client = Fake::with(vec![torrent(hash, 0.5, 512)]);
    remote_client.files = vec![
        TorrentFile {
            name: "duo/a.flac".into(),
            size: 1024,
            progress: 1.0,
        },
        TorrentFile {
            name: "duo/c.flac".into(),
            size: 512,
            progress: 0.0,
        },
    ];
    let dest = tempfile::tempdir().expect("laptop");

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        from: remote.path().to_string_lossy().into_owned(),
        to: dest.path().to_string_lossy().into_owned(),
        save_root: "/data".into(),
        remote: &remote_client,
        partial: PartialMode::CompleteFiles,
        dry_run: false,
        max_torrents: None,
    })
    .expect("fetch");

    assert_eq!(report.fetched, 1);
    assert!(dest.path().join("duo").join("a.flac").is_file());
    assert!(!dest.path().join("duo").join("c.flac").exists());
}

#[test]
fn dry_run_writes_nothing() {
    let (_root, library, staging, remote) = staged();
    let dest = tempfile::tempdir().expect("laptop");
    let remote_client = complete_remote(&library);

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        from: remote.path().to_string_lossy().into_owned(),
        to: dest.path().to_string_lossy().into_owned(),
        save_root: "/data".into(),
        remote: &remote_client,
        partial: PartialMode::Skip,
        dry_run: true,
        max_torrents: None,
    })
    .expect("fetch");

    assert!(report.dry_run);
    assert_eq!(report.fetched, 2);
    assert!(fs::read_dir(dest.path()).expect("dest").next().is_none());
}

#[test]
fn max_torrents_fetches_only_the_smallest() {
    let (_root, library, staging, remote) = staged();
    let dest = tempfile::tempdir().expect("laptop");
    let remote_client = complete_remote(&library);

    let report = run(&Options {
        staging: staging.path().to_path_buf(),
        from: remote.path().to_string_lossy().into_owned(),
        to: dest.path().to_string_lossy().into_owned(),
        save_root: "/data".into(),
        remote: &remote_client,
        partial: PartialMode::Skip,
        dry_run: false,
        max_torrents: Some(1),
    })
    .expect("fetch");

    assert_eq!(report.fetched, 1);
    assert!(dest.path().join("red").join("a.flac").is_file());
    assert!(!dest.path().join("blue").exists());
}

#[test]
fn remote_bytes_are_unchanged() {
    let (_root, library, staging, remote) = staged();
    let dest = tempfile::tempdir().expect("laptop");
    let remote_client = complete_remote(&library);
    let src = remote.path().join("red").join("a.flac");
    let before = fs::read(&src).expect("before");

    run(&Options {
        staging: staging.path().to_path_buf(),
        from: remote.path().to_string_lossy().into_owned(),
        to: dest.path().to_string_lossy().into_owned(),
        save_root: "/data".into(),
        remote: &remote_client,
        partial: PartialMode::Skip,
        dry_run: false,
        max_torrents: None,
    })
    .expect("fetch");

    assert_eq!(fs::read(&src).expect("after"), before);
}
