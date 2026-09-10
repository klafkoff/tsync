//! Rewrite writes staging only. Originals stay put.

use std::fs;

use tsync_core::bencode::{self, Value};
use tsync_core::resume;
use tsync_fixtures::Builder;
use tsync_plan::DEFAULT_BUDGET;
use tsync_rewrite::{Options, run};

fn library() -> (tempfile::TempDir, tsync_fixtures::Library) {
    let root = tempfile::tempdir().expect("tempdir");
    let library = Builder::new()
        .complete("red", "/srv/music", &[("a.flac", 1024)])
        .claims_complete_but_missing("ghost", "/srv/music", &[("g.flac", 4096)], 1)
        .materialize(root.path())
        .expect("materialize");
    (root, library)
}

#[test]
fn staging_gets_rewritten_resume_and_a_copy_of_the_torrent() {
    let (_root, library) = library();
    let staging = tempfile::tempdir().expect("staging");

    let report = run(&Options {
        bt_backup: library.bt_backup.clone(),
        staging: staging.path().to_path_buf(),
        dest: "/data".to_owned(),
        budget: DEFAULT_BUDGET,
        data_root: Some(library.data_root.clone()),
    })
    .expect("rewrite");

    assert_eq!(report.written, 1);
    assert!(report.failed.is_empty());
    assert_eq!(report.plan.excluded.len(), 1);

    let intact = &library.torrents[0];
    let staged_resume = fs::read(
        staging
            .path()
            .join(format!("{}.fastresume", intact.infohash)),
    )
    .expect("staged resume");
    let parsed = resume::parse(&staged_resume).expect("parse staged");
    assert_eq!(parsed.save_path, b"/data");

    let src_torrent = fs::read(&intact.torrent_path).expect("src torrent");
    let dst_torrent =
        fs::read(staging.path().join(format!("{}.torrent", intact.infohash))).expect("dst torrent");
    assert_eq!(src_torrent, dst_torrent);
}

#[test]
fn originals_are_byte_identical_after_rewrite() {
    let (_root, library) = library();
    let staging = tempfile::tempdir().expect("staging");
    let intact = &library.torrents[0];
    let before_resume = fs::read(&intact.resume_path).expect("before resume");
    let before_torrent = fs::read(&intact.torrent_path).expect("before torrent");

    run(&Options {
        bt_backup: library.bt_backup.clone(),
        staging: staging.path().to_path_buf(),
        dest: "/data".to_owned(),
        budget: DEFAULT_BUDGET,
        data_root: Some(library.data_root.clone()),
    })
    .expect("rewrite");

    assert_eq!(
        fs::read(&intact.resume_path).expect("after resume"),
        before_resume
    );
    assert_eq!(
        fs::read(&intact.torrent_path).expect("after torrent"),
        before_torrent
    );
}

#[test]
fn only_path_keys_change_in_the_staged_resume() {
    let (_root, library) = library();
    let staging = tempfile::tempdir().expect("staging");
    let intact = &library.torrents[0];
    let before = fs::read(&intact.resume_path).expect("before");

    run(&Options {
        bt_backup: library.bt_backup.clone(),
        staging: staging.path().to_path_buf(),
        dest: "/data".to_owned(),
        budget: DEFAULT_BUDGET,
        data_root: Some(library.data_root.clone()),
    })
    .expect("rewrite");

    let after = fs::read(
        staging
            .path()
            .join(format!("{}.fastresume", intact.infohash)),
    )
    .expect("after");
    let old = bencode::decode(&before).expect("old");
    let new = bencode::decode(&after).expect("new");
    let old_dict = old.as_dict().expect("dict");
    let new_dict = new.as_dict().expect("dict");
    assert_eq!(old_dict.len(), new_dict.len());
    for (key, value) in old_dict {
        if key == b"save_path" || key == b"qBt-savePath" || key == b"qBt-downloadPath" {
            assert_eq!(
                new_dict.get(key).and_then(Value::as_bytes),
                Some(b"/data".as_slice())
            );
        } else {
            assert_eq!(new_dict.get(key), Some(value));
        }
    }
}

#[test]
fn lockfile_refuses_the_run() {
    let (_root, library) = library();
    let staging = tempfile::tempdir().expect("staging");
    let parent = library.bt_backup.parent().expect("parent");
    fs::write(parent.join("lockfile"), b"held").expect("lock");

    let error = run(&Options {
        bt_backup: library.bt_backup,
        staging: staging.path().to_path_buf(),
        dest: "/data".to_owned(),
        budget: DEFAULT_BUDGET,
        data_root: Some(library.data_root),
    })
    .expect_err("should refuse");

    assert!(matches!(error, tsync_rewrite::Error::ClientRunning(_)));
}
