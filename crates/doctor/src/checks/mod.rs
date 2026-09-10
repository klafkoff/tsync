//! The checks themselves, as data.
//!
//! Each check is a record rather than a branch inside one large function, which
//! is what makes the set enumerable, individually runnable, and extensible
//! without editing a dispatcher.

mod bt_backup;
mod rsync;
mod ssh;

use crate::env::Environment;
use crate::outcome::Outcome;

/// One preflight check.
pub struct Check {
    /// Stable identifier, used in output and to select a single check.
    pub id: &'static str,
    /// One line explaining what this check is for.
    pub description: &'static str,
    /// Inspects the environment and returns a verdict.
    pub probe: fn(&dyn Environment) -> Outcome,
}

/// Checks that inspect the machine `tsync` is running on.
pub const LOCAL: &[Check] = &[
    Check {
        id: "ssh",
        description: "an SSH client for reaching the remote host",
        probe: ssh::check,
    },
    Check {
        id: "rsync",
        description: "GNU rsync new enough to report transfer progress",
        probe: rsync::check,
    },
    Check {
        id: "bt_backup",
        description: "qBittorrent's state directory is readable and listable",
        probe: bt_backup::check,
    },
];

/// Finds a check by its identifier.
#[must_use]
pub fn by_id(id: &str) -> Option<&'static Check> {
    LOCAL.iter().find(|check| check.id == id)
}
