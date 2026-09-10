//! Pure, I/O-free core for `tsync`.
//!
//! Every item in this crate is a function of its inputs: no filesystem, no
//! network, no clock, no global state. That constraint is deliberate. The
//! logic that lives here is the logic where a bug corrupts a torrent silently
//! rather than raising an error, so it must be exhaustively testable without
//! a machine, a peer, or a capture.
//!
//! It is also what keeps the crate reusable. Anything that needs to read or
//! write bencode — the migration pipeline, a transcode packager, a third-party
//! tool — depends on this and nothing else.

pub mod bencode;
pub mod metainfo;
pub mod pathmap;
pub mod resume;
