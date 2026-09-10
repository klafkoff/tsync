//! Fixtures are generated, never captured — and they stay that way.

use std::fs;

use sha1::Digest;
use tsync_core::bencode::{self, Value};
use tsync_fixtures::Builder;

const HEX: &[u8; 16] = b"0123456789abcdef";

#[test]
fn paired_backup_files_are_named_by_infohash() {
    let root = tempfile::tempdir().expect("tempdir");
    let library = Builder::new()
        .complete(
            "red-album",
            "/srv/music",
            &[("01.flac", 32_768), ("02.flac", 16_384)],
        )
        .materialize(root.path())
        .expect("materialize");

    assert_eq!(library.torrents.len(), 1);
    let torrent = &library.torrents[0];
    assert_eq!(torrent.infohash.len(), 40);
    assert!(
        torrent
            .infohash
            .chars()
            .all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "infohash must be lowercase hex"
    );
    assert_eq!(
        torrent.torrent_path.file_name().unwrap(),
        format!("{}.torrent", torrent.infohash).as_str()
    );
    assert!(torrent.torrent_path.is_file());
    assert!(torrent.resume_path.is_file());
}

#[test]
fn torrent_and_resume_round_trip_through_strict_bencode() {
    let root = tempfile::tempdir().expect("tempdir");
    let library = Builder::new()
        .complete("album", "/srv/music", &[("a.flac", 4096)])
        .materialize(root.path())
        .expect("materialize");

    let torrent_bytes = fs::read(&library.torrents[0].torrent_path).expect("read torrent");
    let resume_bytes = fs::read(&library.torrents[0].resume_path).expect("read resume");

    let torrent = bencode::decode(&torrent_bytes).expect("torrent must be canonical");
    let resume = bencode::decode(&resume_bytes).expect("resume must be canonical");

    assert_eq!(bencode::encode(&torrent), torrent_bytes);
    assert_eq!(bencode::encode(&resume), resume_bytes);
}

#[test]
fn infohash_is_sha1_of_the_encoded_info_dict() {
    let root = tempfile::tempdir().expect("tempdir");
    let library = Builder::new()
        .complete("album", "/srv/music", &[("a.flac", 4096)])
        .materialize(root.path())
        .expect("materialize");

    let bytes = fs::read(&library.torrents[0].torrent_path).expect("read");
    let value = bencode::decode(&bytes).expect("decode");
    let info = value.get(b"info").expect("info dict");
    let digest = sha1::Sha1::digest(bencode::encode(info));
    let mut expected = String::with_capacity(40);
    for byte in digest {
        expected.push(HEX[usize::from(byte >> 4)] as char);
        expected.push(HEX[usize::from(byte & 0x0f)] as char);
    }

    assert_eq!(library.torrents[0].infohash, expected);
}

#[test]
fn same_description_produces_the_same_infohash() {
    let a = tempfile::tempdir().expect("tempdir");
    let b = tempfile::tempdir().expect("tempdir");

    let build = || Builder::new().complete("album", "/srv/music", &[("a.flac", 8192)]);

    let left = build().materialize(a.path()).expect("a");
    let right = build().materialize(b.path()).expect("b");

    assert_eq!(left.torrents[0].infohash, right.torrents[0].infohash);
}

#[test]
fn claims_complete_but_missing_omits_the_named_files() {
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

    let base = library.data_root.join("srv/music/ghost");
    assert!(!base.join("gone.flac").exists());
    assert!(base.join("kept.flac").is_file());
    assert_eq!(fs::read(base.join("kept.flac")).expect("read").len(), 4096);

    let resume = fs::read(&library.torrents[0].resume_path).expect("read resume");
    let value = bencode::decode(&resume).expect("decode");
    assert_eq!(
        value.get(b"save_path").and_then(Value::as_bytes),
        Some(b"/srv/music".as_slice())
    );
}

#[test]
fn generated_bytes_never_contain_a_real_home_path() {
    let root = tempfile::tempdir().expect("tempdir");
    let library = Builder::new()
        .complete("album", "/srv/music", &[("a.flac", 1024)])
        .materialize(root.path())
        .expect("materialize");

    for torrent in &library.torrents {
        for path in [&torrent.torrent_path, &torrent.resume_path] {
            let bytes = fs::read(path).expect("read");
            assert!(
                !contains_slice(&bytes, b"/Users/"),
                "{} must not contain a real home prefix",
                path.display()
            );
            assert!(
                !contains_slice(&bytes, b"/home/"),
                "{} must not contain a real home prefix",
                path.display()
            );
        }
    }
}

fn contains_slice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
