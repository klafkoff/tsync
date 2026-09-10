//! Can qBittorrent's state directory actually be read?
//!
//! `BT_backup` holds a `.torrent` and a `.fastresume` for every torrent in the
//! library. It is the input to everything this tool does.
//!
//! The check performs a real directory listing rather than an existence test,
//! because on macOS those are different permissions. Under TCC, `stat` on a
//! known path inside a protected directory can succeed while listing its parent
//! is denied. Code that checks existence therefore reports everything as fine
//! and then finds nothing to work on — a confusing partial view instead of a
//! clean error. Listing is the operation that actually gets used, so listing is
//! the operation that gets checked.

use std::io;
use std::path::{Path, PathBuf};

use crate::env::{Environment, Platform};
use crate::outcome::Outcome;

const RELOCATE_HINT: &str = "if qBittorrent keeps its state elsewhere, pass --bt-backup";

pub(crate) fn check(env: &dyn Environment) -> Outcome {
    let Some(home) = env.home() else {
        return Outcome::warn("home directory unknown, cannot locate BT_backup");
    };
    let path = default_path(env.platform(), &home);

    match env.list_dir(&path) {
        Ok(entries) => describe_contents(&path, &entries),

        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Outcome::warn(format!("not found at {}", path.display()))
                .expected("qBittorrent's BT_backup directory")
                .fix(RELOCATE_HINT)
        }

        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            let outcome = Outcome::fail(format!("cannot list {}", path.display()))
                .expected("permission to list the directory");

            // Naming Full Disk Access matters. A generic "permission denied"
            // sends people to chmod, which cannot fix a TCC denial.
            match env.platform() {
                Platform::MacOs => outcome.fix(
                    "grant Full Disk Access to your terminal in \
                     System Settings > Privacy & Security, then restart it",
                ),
                Platform::Linux | Platform::Other => {
                    outcome.fix("adjust the directory's permissions")
                }
            }
        }

        Err(error) => Outcome::fail(format!("cannot list {}: {error}", path.display())),
    }
}

fn describe_contents(path: &Path, entries: &[PathBuf]) -> Outcome {
    let torrents = entries
        .iter()
        .filter(|entry| entry.extension().is_some_and(|ext| ext == "torrent"))
        .count();

    if torrents == 0 {
        // Listable but empty is not a permission problem. Passing with "0
        // torrents" would read as though the library were empty, when the more
        // likely explanation is that this is the wrong directory.
        return Outcome::warn(format!(
            "{} is listable but holds no torrents",
            path.display()
        ))
        .expected("at least one .torrent file")
        .fix(RELOCATE_HINT);
    }

    Outcome::pass(format!("readable, {torrents} torrents"))
}

fn default_path(platform: Platform, home: &Path) -> PathBuf {
    match platform {
        Platform::MacOs => home.join("Library/Application Support/qBittorrent/BT_backup"),
        Platform::Linux | Platform::Other => home.join(".local/share/qBittorrent/BT_backup"),
    }
}
