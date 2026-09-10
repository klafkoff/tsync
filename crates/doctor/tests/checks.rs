//! Checks run against recorded output from real tools.
//!
//! The banners below are verbatim strings these programs actually print. That
//! is the point of the corpus: identification logic breaks on real-world
//! variation, not on the tidy output a test author would invent.

use std::io;

use tsync_doctor::env::{Fake, Platform};
use tsync_doctor::outcome::Status;
use tsync_doctor::{Report, checks};

/// GNU rsync, current.
const GNU_3_2_7: &str = "\
rsync  version 3.2.7  protocol version 31
Copyright (C) 1996-2022 by Andrew Tridgell, Wayne Davison, and others.
Web site: https://rsync.samba.org/
";

/// GNU rsync, older than the `--info` flag.
const GNU_2_6_9: &str = "\
rsync  version 2.6.9  protocol version 29
Copyright (C) 1996-2006 by Andrew Tridgell, Wayne Davison, and others.
";

/// What ships as `/usr/bin/rsync` on current macOS.
///
/// This is the regression test that keeps anyone from reintroducing version
/// parsing: the string "rsync version 2.6.9" appears here, and it is not GNU
/// rsync 2.6.9.
const OPENRSYNC: &str = "\
openrsync: protocol version 29
rsync version 2.6.9 compatible
";

fn run(id: &str, env: &Fake) -> tsync_doctor::Outcome {
    let check = checks::by_id(id).expect("check exists");
    (check.probe)(env)
}

// ---------------------------------------------------------------- rsync

#[test]
fn rsync_passes_when_the_capability_probe_succeeds() {
    let env = Fake::new(Platform::MacOs)
        .command("rsync --version", GNU_3_2_7)
        .command("rsync --info=help", "Use OPT or ALL");

    let outcome = run("rsync", &env);

    assert_eq!(outcome.status, Status::Pass);
    assert_eq!(outcome.found, "GNU rsync 3.2.7");
}

#[test]
fn rsync_identifies_openrsync_by_name_rather_than_version() {
    // --info=help is deliberately unregistered: openrsync does not have it.
    let env = Fake::new(Platform::MacOs).command("rsync --version", OPENRSYNC);

    let outcome = run("rsync", &env);

    assert_eq!(outcome.status, Status::Fail);
    assert_eq!(
        outcome.found, "openrsync (2.6.9-compatible)",
        "must name the implementation; reporting a bare 2.6.9 sends people \
         looking for an upgrade that does not exist"
    );
    assert_eq!(outcome.remediation.as_deref(), Some("brew install rsync"));
}

#[test]
fn rsync_fails_on_genuinely_old_gnu_rsync() {
    let env = Fake::new(Platform::Linux).command("rsync --version", GNU_2_6_9);

    let outcome = run("rsync", &env);

    assert_eq!(outcome.status, Status::Fail);
    assert_eq!(outcome.found, "GNU rsync 2.6.9");
    assert!(
        outcome
            .remediation
            .as_deref()
            .is_some_and(|fix| fix.contains("distribution")),
        "remediation must suit the platform under test, not the host"
    );
}

#[test]
fn rsync_points_at_homebrew_when_path_still_has_openrsync() {
    let env = Fake::new(Platform::MacOs)
        .command("rsync --version", OPENRSYNC)
        .command("/opt/homebrew/bin/rsync --info=help", "Use OPT or ALL");

    let outcome = run("rsync", &env);

    assert_eq!(outcome.status, Status::Fail);
    assert!(
        outcome
            .remediation
            .as_deref()
            .is_some_and(|fix| fix.contains("/opt/homebrew/bin") && fix.contains("PATH")),
        "when GNU rsync is already installed, say so instead of 'brew install'"
    );
}

#[test]
fn rsync_fails_when_absent() {
    let env = Fake::new(Platform::MacOs).missing("rsync");

    let outcome = run("rsync", &env);

    assert_eq!(outcome.status, Status::Fail);
    assert_eq!(outcome.found, "not found on PATH");
    assert_eq!(outcome.remediation.as_deref(), Some("brew install rsync"));
}

#[test]
fn rsync_trusts_the_probe_over_an_unparseable_banner() {
    // A fork with an unfamiliar banner is still usable if the flag works.
    let env = Fake::new(Platform::Linux)
        .command("rsync --version", "some fork of rsync, no version line\n")
        .command("rsync --info=help", "Use OPT or ALL");

    assert_eq!(run("rsync", &env).status, Status::Pass);
}

// ------------------------------------------------------------------ ssh

#[test]
fn ssh_reports_the_openssh_product() {
    let env = Fake::new(Platform::MacOs).command_exit(
        "ssh -V",
        0,
        "",
        "OpenSSH_9.8p1, LibreSSL 3.3.6\n", // ssh -V writes to stderr
    );

    let outcome = run("ssh", &env);

    assert_eq!(outcome.status, Status::Pass);
    assert_eq!(outcome.found, "OpenSSH_9.8p1");
}

#[test]
fn ssh_fails_when_absent() {
    let env = Fake::new(Platform::Linux).missing("ssh");

    assert_eq!(run("ssh", &env).status, Status::Fail);
}

// ------------------------------------------------------------ bt_backup

const MAC_BT_BACKUP: &str = "/Users/test/Library/Application Support/qBittorrent/BT_backup";

#[test]
fn bt_backup_counts_torrents_when_listable() {
    let env = Fake::new(Platform::MacOs).home("/Users/test").dir(
        MAC_BT_BACKUP,
        &["a.torrent", "a.fastresume", "b.torrent", "b.fastresume"],
    );

    let outcome = run("bt_backup", &env);

    assert_eq!(outcome.status, Status::Pass);
    assert_eq!(outcome.found, "readable, 2 torrents");
}

#[test]
fn bt_backup_names_full_disk_access_when_macos_denies_the_listing() {
    let env = Fake::new(Platform::MacOs)
        .home("/Users/test")
        .dir_error(MAC_BT_BACKUP, io::ErrorKind::PermissionDenied);

    let outcome = run("bt_backup", &env);

    assert_eq!(outcome.status, Status::Fail);
    assert!(
        outcome
            .remediation
            .as_deref()
            .is_some_and(|fix| fix.contains("Full Disk Access")),
        "a generic permission message sends people to chmod, which cannot fix \
         a TCC denial"
    );
}

#[test]
fn bt_backup_warns_rather_than_fails_when_the_directory_is_missing() {
    // Absent is not the same as forbidden: qBittorrent may simply live
    // elsewhere, which is a configuration matter rather than a broken machine.
    let env = Fake::new(Platform::MacOs).home("/Users/test");

    assert_eq!(run("bt_backup", &env).status, Status::Warn);
}

#[test]
fn bt_backup_warns_when_listable_but_holding_no_torrents() {
    let env = Fake::new(Platform::Linux)
        .home("/home/test")
        .dir("/home/test/.local/share/qBittorrent/BT_backup", &["lock"]);

    assert_eq!(run("bt_backup", &env).status, Status::Warn);
}

// --------------------------------------------------------------- report

#[test]
fn warnings_do_not_block_but_failures_do() {
    let warning_only = Fake::new(Platform::MacOs)
        .home("/Users/test")
        .command_exit("ssh -V", 0, "", "OpenSSH_9.8p1, LibreSSL 3.3.6\n")
        .command("rsync --version", GNU_3_2_7)
        .command("rsync --info=help", "Use OPT or ALL");

    let report = Report::run("local", checks::LOCAL, &warning_only);
    assert_eq!(report.count(Status::Warn), 1, "bt_backup is absent");
    assert!(
        !report.is_blocking(),
        "a preflight that halts on warnings stops being run"
    );

    let with_failure = Fake::new(Platform::MacOs)
        .home("/Users/test")
        .command("rsync --version", OPENRSYNC);

    assert!(Report::run("local", checks::LOCAL, &with_failure).is_blocking());
}

#[test]
fn rendering_includes_the_remediation() {
    let env = Fake::new(Platform::MacOs).command("rsync --version", OPENRSYNC);
    let rendered = Report::run("local", checks::LOCAL, &env).render();

    assert!(rendered.contains("FAIL  rsync"));
    assert!(rendered.contains("fix: brew install rsync"));
    assert!(rendered.contains("1 failed"));
}
