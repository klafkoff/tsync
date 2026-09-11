//! Plan is derived from fixtures, never a real library.

use tsync_audit::{Class, Options, run};
use tsync_fixtures::Builder;
use tsync_plan::{DEFAULT_BUDGET, build};

fn manifest(builder: Builder) -> tsync_audit::Manifest {
    let root = tempfile::tempdir().expect("tempdir");
    let library = builder.materialize(root.path()).expect("materialize");
    run(&Options {
        bt_backup: library.bt_backup,
        data_root: Some(library.data_root),
    })
    .expect("audit")
}

#[test]
fn intact_torrents_are_rewritten_under_the_destination() {
    let manifest = manifest(
        Builder::new()
            .complete("red", "/srv/music", &[("a.flac", 1024)])
            .complete("blue", "/srv/music/extra", &[("b.flac", 2048)]),
    );

    let plan = build(&manifest, "/data", DEFAULT_BUDGET).expect("plan");

    assert_eq!(plan.mapping.rules().len(), 1);
    assert_eq!(plan.mapping.rules()[0].from_display(), "/srv/music");
    assert_eq!(plan.eligible_count(), 2);
    assert!(plan.excluded.is_empty());

    let dests: Vec<_> = plan
        .batches
        .iter()
        .flat_map(|batch| batch.torrents.iter())
        .map(|item| item.dest_path.as_str())
        .collect();
    assert!(dests.contains(&"/data"));
    assert!(dests.contains(&"/data/extra"));
}

#[test]
fn partials_are_excluded_and_listed() {
    let manifest = manifest(
        Builder::new()
            .complete("ok", "/srv/music", &[("a.flac", 1024)])
            .claims_complete_but_missing("ghost", "/srv/music", &[("g.flac", 4096)], 1),
    );

    let plan = build(&manifest, "/data", DEFAULT_BUDGET).expect("plan");

    assert_eq!(plan.eligible_count(), 1);
    assert_eq!(plan.excluded.len(), 1);
    assert!(matches!(
        plan.excluded[0].entry.class,
        Class::Partial {
            disagreement: true,
            ..
        }
    ));
    assert!(plan.render().contains("excluded"));
    assert!(plan.render().contains("resume claims complete"));
}

#[test]
fn oversized_torrent_is_its_own_batch() {
    let manifest = manifest(
        Builder::new()
            .complete("tiny", "/srv/music", &[("a.flac", 100)])
            .complete("huge", "/srv/music", &[("b.flac", 1000)]),
    );

    let plan = build(&manifest, "/data", 200).expect("plan");

    assert_eq!(plan.batches.len(), 2);
    assert_eq!(plan.batches[0].torrents.len(), 1);
    assert_eq!(plan.batches[0].torrents[0].entry.name, "tiny");
    assert_eq!(plan.batches[1].torrents[0].entry.name, "huge");
}

#[test]
fn skipped_torrents_are_eligible() {
    let manifest = manifest(Builder::new().skipped(
        "album",
        "/srv/music",
        &[("keep.flac", 1024), ("skip.flac", 1024)],
        &[1],
    ));

    let plan = build(&manifest, "/data", DEFAULT_BUDGET).expect("plan");
    assert_eq!(plan.eligible_count(), 1);
    assert!(plan.excluded.is_empty());
}

#[test]
fn take_smallest_stops_before_exceeding_the_byte_cap() {
    let manifest = manifest(
        Builder::new()
            .complete("tiny", "/srv/music", &[("a.flac", 100)])
            .complete("mid", "/srv/music", &[("b.flac", 200)])
            .complete("big", "/srv/music", &[("c.flac", 400)]),
    );

    let plan = build(&manifest, "/data", DEFAULT_BUDGET)
        .expect("plan")
        .take_smallest(Some(350), None);

    assert_eq!(plan.eligible_count(), 2);
    assert_eq!(plan.eligible_bytes(), 300);
    let names: Vec<_> = plan
        .batches
        .iter()
        .flat_map(|batch| batch.torrents.iter())
        .map(|item| item.entry.name.as_str())
        .collect();
    assert_eq!(names, ["tiny", "mid"]);
}

#[test]
fn take_smallest_honors_a_count_cap() {
    let manifest = manifest(
        Builder::new()
            .complete("a", "/srv/music", &[("a.flac", 10)])
            .complete("b", "/srv/music", &[("b.flac", 20)])
            .complete("c", "/srv/music", &[("c.flac", 30)]),
    );

    let plan = build(&manifest, "/data", DEFAULT_BUDGET)
        .expect("plan")
        .take_smallest(None, Some(1));

    assert_eq!(plan.eligible_count(), 1);
    assert_eq!(plan.batches[0].torrents[0].entry.name, "a");
}

#[test]
fn render_is_a_dry_run_and_names_the_rewrite() {
    let manifest = manifest(Builder::new().complete("red", "/srv/music", &[("a.flac", 1024)]));
    let text = build(&manifest, "/opt/seedbox/data", DEFAULT_BUDGET)
        .expect("plan")
        .render();

    assert!(text.contains("dry run"));
    assert!(text.contains("/srv/music"));
    assert!(text.contains("/opt/seedbox/data"));
    assert!(text.contains("rewrites"));
}
