//! Preflight checks for `tsync`.
//!
//! Every failure mode in this project is cheaper to catch before a seven-hour
//! transfer than during one. This crate is the machinery for finding them
//! first.
//!
//! Two rules shape everything here.
//!
//! **Probe for the capability, not the version number.** Version strings lie —
//! most memorably `openrsync`, which reports itself as "rsync version 2.6.9
//! compatible" while being an entirely different program. Asking whether a
//! feature works answers the question actually being asked, and degrades
//! gracefully across forks and future releases in a way a version floor does
//! not.
//!
//! **Checks are data over an injected environment.** A check is a record with
//! an id, a description, and a probe that may only touch the
//! [`Environment`](env::Environment) it is handed. That makes the whole suite
//! testable against recorded output with no real host, and it means these same
//! checks will run against a remote machine once an SSH-backed environment
//! exists.

pub mod checks;
pub mod env;
pub mod outcome;
pub mod report;

pub use checks::Check;
pub use env::{Environment, Host, Platform};
pub use outcome::{Outcome, Status};
pub use report::Report;
