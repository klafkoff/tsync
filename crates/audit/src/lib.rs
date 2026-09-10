//! Inventory a torrent library without touching its data.
//!
//! Walk a `BT_backup`, pair each `.torrent` with its `.fastresume`, reconstruct
//! every expected path, and `stat` it. Classification is observed size versus
//! declared length — existence alone is not enough, because a truncated file
//! exists and would only fail after you had already copied it.
//!
//! The result is a manifest: infohashes, sizes, save paths, and states. Announce
//! URLs never leave the parser. Nothing on disk is modified.

pub mod classify;
pub mod manifest;
pub mod paths;
pub mod scan;

pub use classify::Class;
pub use manifest::{Entry, Manifest};
pub use scan::{Error, Options, run};
