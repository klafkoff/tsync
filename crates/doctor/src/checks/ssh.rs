//! Is there an SSH client to reach the remote host with?

use crate::env::{Environment, Platform};
use crate::outcome::Outcome;

pub(crate) fn check(env: &dyn Environment) -> Outcome {
    // `ssh -V` writes to stderr, so the exit code carries no information here
    // and the banner is the whole result.
    let Ok(output) = env.run("ssh", &["-V"]) else {
        return Outcome::fail("not found on PATH")
            .expected("an OpenSSH client")
            .fix(install_hint(env.platform()));
    };

    match parse_version(&output.combined()) {
        Some(version) => Outcome::pass(version),
        None => Outcome::warn("present, but did not report a version"),
    }
}

fn install_hint(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => "install the Xcode command line tools: xcode-select --install",
        Platform::Linux => "install the openssh-client package from your distribution",
        Platform::Other => "install an OpenSSH client",
    }
}

/// Extracts the product from a banner such as `OpenSSH_9.8p1, LibreSSL 3.3.6`.
fn parse_version(banner: &str) -> Option<String> {
    let token = banner.split_whitespace().next()?.trim_end_matches(',');

    // Guard against a first token that is not a version at all, which would
    // otherwise be reported as though it were one.
    (token.starts_with("OpenSSH") || token.contains('_')).then(|| token.to_owned())
}
