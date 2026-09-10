//! Is there an rsync that can report transfer progress?
//!
//! This check has the shape it does because of one specific trap. On current
//! macOS, `/usr/bin/rsync` is `openrsync`, and its banner reads:
//!
//! ```text
//! openrsync: protocol version 29
//! rsync version 2.6.9 compatible
//! ```
//!
//! Anything that scrapes a number out of that finds `2.6.9` and concludes it is
//! looking at an old GNU rsync. It is not — it is a different implementation
//! advertising compatibility with an old protocol. A version floor rejects this
//! particular banner correctly, but only by accident, and it would wrongly
//! accept an `openrsync` that claimed a higher number.
//!
//! So the check probes the capability instead. `--info=help` exists only on GNU
//! rsync 3.1.0 and later, which is exactly the threshold that matters, because
//! `--info=progress2` is how a transfer reports aggregate progress. The version
//! is parsed only to write a message a human can act on.

use crate::env::{Environment, Platform};
use crate::outcome::Outcome;

const REQUIREMENT: &str = "GNU rsync >= 3.1.0, for --info=progress2";

pub(crate) fn check(env: &dyn Environment) -> Outcome {
    let Ok(banner) = env.run("rsync", &["--version"]) else {
        return Outcome::fail("not found on PATH")
            .expected(REQUIREMENT)
            .fix(install_hint(env.platform()));
    };

    let identity = Identity::parse(&banner.combined());

    // The question is not "which version is this" but "can it do the thing".
    let capable = env
        .run("rsync", &["--info=help"])
        .is_ok_and(|probe| probe.succeeded());

    if capable {
        return Outcome::pass(identity.describe());
    }

    // Homebrew installs GNU rsync but does not replace /usr/bin/rsync.
    // If PATH still prefers openrsync, the capability probe fails even
    // though the right binary is already on the disk.
    let outcome = Outcome::fail(identity.describe()).expected(REQUIREMENT);
    if let Some(path) = gnu_rsync_elsewhere(env) {
        let bin_dir = std::path::Path::new(&path)
            .parent()
            .map_or_else(|| path.clone(), |parent| parent.display().to_string());
        return outcome.fix(format!(
            "GNU rsync is at {path}; put {bin_dir} ahead of /usr/bin on PATH"
        ));
    }

    outcome.fix(install_hint(env.platform()))
}

/// Well-known locations Homebrew uses, tried only when `rsync` on PATH failed.
fn gnu_rsync_elsewhere(env: &dyn Environment) -> Option<String> {
    const CANDIDATES: &[&str] = &["/opt/homebrew/bin/rsync", "/usr/local/bin/rsync"];
    for path in CANDIDATES {
        if env
            .run(path, &["--info=help"])
            .is_ok_and(|probe| probe.succeeded())
        {
            return Some((*path).to_owned());
        }
    }
    None
}

fn install_hint(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "brew install rsync",
        Platform::Linux => "install the rsync package from your distribution",
        Platform::Other => "install GNU rsync 3.1.0 or later",
    }
}

/// Which rsync this is, and what it calls itself.
struct Identity {
    implementation: Implementation,
    version: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Implementation {
    Gnu,
    OpenRsync,
    Unknown,
}

impl Identity {
    fn parse(banner: &str) -> Self {
        let implementation = if banner.contains("openrsync") {
            Implementation::OpenRsync
        } else if banner.contains("rsync") {
            Implementation::Gnu
        } else {
            Implementation::Unknown
        };

        Self {
            implementation,
            version: parse_version(banner),
        }
    }

    fn describe(&self) -> String {
        match (self.implementation, self.version.as_deref()) {
            // Naming the implementation is the entire point. Reporting "2.6.9"
            // alone sends people hunting for an upgrade that does not exist.
            (Implementation::OpenRsync, Some(version)) => {
                format!("openrsync ({version}-compatible)")
            }
            (Implementation::OpenRsync, None) => "openrsync".to_owned(),
            (Implementation::Gnu, Some(version)) => format!("GNU rsync {version}"),
            (Implementation::Gnu, None) => "rsync, version not reported".to_owned(),
            (Implementation::Unknown, _) => "unrecognized rsync".to_owned(),
        }
    }
}

/// Pulls the version out of a banner.
///
/// Both implementations print a line beginning with `rsync` carrying the
/// version after the word `version`. `openrsync` prints an earlier line —
/// `openrsync: protocol version 29` — that would yield the protocol number
/// instead, so only lines that *start* with `rsync` are considered.
fn parse_version(banner: &str) -> Option<String> {
    let line = banner
        .lines()
        .map(str::trim_start)
        .find(|line| line.starts_with("rsync") && line.contains("version"))?;

    let mut tokens = line
        .split_whitespace()
        .skip_while(|&token| token != "version");
    tokens.next()?; // the word "version" itself
    let candidate = tokens.next()?;

    candidate
        .starts_with(|c: char| c.is_ascii_digit())
        .then(|| candidate.to_owned())
}
