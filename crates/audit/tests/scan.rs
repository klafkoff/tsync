//! Audit against generated libraries, never a real one.

use std::fs;

use tsync_audit::{Class, Options, run};
use tsync_fixtures::Builder;

#[test]
fn complete_fixture_is_intact() {
    let root = tempfile::tempdir().expect("tempdir");
    let library = Builder::new()
        .complete(
            "red-album",
            "/srv/music",
            &[("01.flac", 32_768), ("02.flac", 16_384)],
        )
        .materialize(root.path())
        .expect("materialize");

    let manifest = run(&Options {
        bt_backup: library.bt_backup,
        data_root: Some(library.data_root),
    })
    .expect("scan");

    assert_eq!(manifest.torrents.len(), 1);
    assert_eq!(manifest.torrents[0].class, Class::Intact);
    assert_eq!(manifest.torrents[0].name, "red-album");
    assert_eq!(manifest.torrents[0].save_path, "/srv/music");
}

#[test]
fn missing_files_that_claim_complete_are_flagged() {
    let root = tempfile::tempdir().expect("tempdir");
    let library = Builder::new()
        .claims_complete_but_missing(
            "ghost",
            "/srv/music",
            &[("gone.flac", 4096), ("kept.flac", 4096)],
            1,
        )
        .materialize(root.path())
        .expect("materialize");

    let manifest = run(&Options {
        bt_backup: library.bt_backup,
        data_root: Some(library.data_root),
    })
    .expect("scan");

    assert_eq!(
        manifest.torrents[0].class,
        Class::Partial {
            missing: 1,
            truncated: 0,
            disagreement: true,
        }
    );
}

#[test]
fn truncated_file_is_partial_not_intact() {
    let root = tempfile::tempdir().expect("tempdir");
    let library = Builder::new()
        .truncated("short", "/srv/music", &[("a.flac", 8192)], 0, 100)
        .materialize(root.path())
        .expect("materialize");

    let manifest = run(&Options {
        bt_backup: library.bt_backup,
        data_root: Some(library.data_root),
    })
    .expect("scan");

    assert_eq!(
        manifest.torrents[0].class,
        Class::Partial {
            missing: 0,
            truncated: 1,
            disagreement: true,
        }
    );
}

#[test]
fn deselected_files_are_skipped() {
    let root = tempfile::tempdir().expect("tempdir");
    let library = Builder::new()
        .skipped(
            "album",
            "/srv/music",
            &[("keep.flac", 4096), ("skip.flac", 4096)],
            &[1],
        )
        .materialize(root.path())
        .expect("materialize");

    let manifest = run(&Options {
        bt_backup: library.bt_backup,
        data_root: Some(library.data_root),
    })
    .expect("scan");

    assert_eq!(manifest.torrents[0].class, Class::Skipped);
}

#[test]
fn unpaired_backup_files_are_counted_not_classified() {
    let root = tempfile::tempdir().expect("tempdir");
    let library = Builder::new()
        .complete("album", "/srv/music", &[("a.flac", 1024)])
        .materialize(root.path())
        .expect("materialize");

    fs::write(library.bt_backup.join("deadbeef.torrent"), b"not-bencode").expect("orphan torrent");
    fs::write(library.bt_backup.join("cafebabe.fastresume"), b"x").expect("orphan resume");

    let manifest = run(&Options {
        bt_backup: library.bt_backup,
        data_root: Some(library.data_root),
    })
    .expect("scan");

    assert_eq!(manifest.unpaired_torrent, 1);
    assert_eq!(manifest.unpaired_resume, 1);
    assert_eq!(manifest.torrents.len(), 1);
}

#[test]
fn manifest_does_not_contain_the_announce_url() {
    let root = tempfile::tempdir().expect("tempdir");
    let library = Builder::new()
        .complete("album", "/srv/music", &[("a.flac", 1024)])
        .materialize(root.path())
        .expect("materialize");

    let rendered = run(&Options {
        bt_backup: library.bt_backup,
        data_root: Some(library.data_root),
    })
    .expect("scan")
    .render();

    assert!(
        !rendered.contains("announce"),
        "report must not mention announce URLs"
    );
    assert!(
        !rendered.contains("tracker.invalid"),
        "report must not leak the announce host"
    );
}
