//! Transfer against generated libraries, never a real one.

use std::fs;
use std::path::Path;

use tsync_fixtures::Builder;
use tsync_plan::DEFAULT_BUDGET;
use tsync_transfer::{Options, run};

fn library() -> (tempfile::TempDir, tsync_fixtures::Library) {
    let root = tempfile::tempdir().expect("tempdir");
    let library = Builder::new()
        .complete("red", "/srv/music", &[("a.flac", 1024)])
        .complete("blue", "/srv/music", &[("b.flac", 2048)])
        .claims_complete_but_missing("ghost", "/srv/music", &[("g.flac", 4096)], 1)
        .materialize(root.path())
        .expect("materialize");
    (root, library)
}

fn opts(
    library: &tsync_fixtures::Library,
    dest: &Path,
    max_bytes: Option<u64>,
    max_torrents: Option<usize>,
    dry_run: bool,
) -> Options {
    Options {
        bt_backup: library.bt_backup.clone(),
        dest: dest.to_string_lossy().into_owned(),
        budget: DEFAULT_BUDGET,
        data_root: Some(library.data_root.clone()),
        max_bytes,
        max_torrents,
        dry_run,
    }
}

#[test]
fn copies_intact_torrents_and_skips_partials() {
    let (_root, library) = library();
    let dest = tempfile::tempdir().expect("dest");

    let report = run(&opts(&library, dest.path(), None, None, false)).expect("transfer");

    assert_eq!(report.copied, 2);
    assert!(report.failed.is_empty());
    assert_eq!(report.plan.excluded.len(), 1);
    assert_eq!(report.deferred, 0);

    let red = dest.path().join("red").join("a.flac");
    let blue = dest.path().join("blue").join("b.flac");
    assert_eq!(fs::metadata(&red).expect("red").len(), 1024);
    assert_eq!(fs::metadata(&blue).expect("blue").len(), 2048);
    assert!(!dest.path().join("ghost").exists());
}

#[test]
fn source_bytes_are_unchanged() {
    let (_root, library) = library();
    let dest = tempfile::tempdir().expect("dest");
    let src = library.data_root.join("srv/music/red/a.flac");
    let before = fs::read(&src).expect("before");

    run(&opts(&library, dest.path(), None, None, false)).expect("transfer");

    assert_eq!(fs::read(&src).expect("after"), before);
    let dest_file = dest.path().join("red").join("a.flac");
    assert_eq!(fs::read(&dest_file).expect("copied"), before);
}

#[test]
fn max_torrents_copies_only_the_smallest() {
    let (_root, library) = library();
    let dest = tempfile::tempdir().expect("dest");

    let report = run(&opts(&library, dest.path(), None, Some(1), false)).expect("transfer");

    assert_eq!(report.copied, 1);
    assert_eq!(report.deferred, 1);
    assert!(dest.path().join("red").join("a.flac").exists());
    assert!(!dest.path().join("blue").exists());
}

#[test]
fn max_bytes_stops_before_exceeding() {
    let (_root, library) = library();
    let dest = tempfile::tempdir().expect("dest");

    let report = run(&opts(&library, dest.path(), Some(1500), None, false)).expect("transfer");

    assert_eq!(report.copied, 1);
    assert_eq!(report.plan.eligible_bytes(), 1024);
    assert!(dest.path().join("red").join("a.flac").exists());
    assert!(!dest.path().join("blue").exists());
}

#[test]
fn dry_run_writes_nothing() {
    let (_root, library) = library();
    let dest = tempfile::tempdir().expect("dest");

    let report = run(&opts(&library, dest.path(), None, None, true)).expect("transfer");

    assert!(report.dry_run);
    assert_eq!(report.copied, 0);
    assert_eq!(report.plan.eligible_count(), 2);
    assert!(fs::read_dir(dest.path()).expect("dest").next().is_none());
}

#[test]
fn render_does_not_include_torrent_names() {
    let (_root, library) = library();
    let dest = tempfile::tempdir().expect("dest");
    let report = run(&opts(&library, dest.path(), None, None, true)).expect("transfer");
    let text = report.render();
    assert!(text.contains("dry run"));
    assert!(text.contains("selected"));
    assert!(!text.contains("red"));
    assert!(!text.contains("blue"));
    assert!(!text.contains("ghost"));
}
