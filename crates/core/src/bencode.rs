//! Canonical bencode encoding and decoding.
//!
//! Bencode is the serialization format used by `.torrent` files and by
//! qBittorrent's `.fastresume` resume data. It has four types: integers, byte
//! strings, lists, and dictionaries whose keys are byte strings sorted in
//! ascending byte order.
//!
//! # The invariant this module exists to provide
//!
//! Rewriting resume data means decoding a file, changing a handful of path
//! fields, and writing it back. Every byte that was not deliberately changed
//! must survive untouched. So the property that matters is:
//!
//! ```text
//! encode(decode(bytes)) == bytes
//! ```
//!
//! Holding that for *all* inputs is impossible, because bencode permits
//! non-canonical spellings — `i03e` and `i3e` decode to the same integer but
//! are different bytes, and a dictionary can be written with unsorted keys.
//! A canonical encoder would silently rewrite those, changing bytes nobody
//! asked to change.
//!
//! # Strictness
//!
//! This decoder therefore **rejects non-canonical input** rather than
//! normalizing it. Specifically it refuses:
//!
//! - integers with leading zeros (`i03e`) or negative zero (`i-0e`)
//! - byte string lengths with leading zeros (`03:abc`)
//! - dictionary keys that are not in strictly ascending byte order, which
//!   catches both misordering and duplicates
//! - trailing data after the top-level value
//! - nesting deeper than [`MAX_DEPTH`]
//!
//! With those rejected, the round-trip invariant holds unconditionally for
//! every input this module accepts, and any file that would have violated it
//! fails loudly at parse time instead of being quietly rewritten.
//!
//! libtorrent and qBittorrent both emit canonical bencode, so strictness costs
//! nothing in practice and converts a class of silent corruption into an error.

mod decode;
mod encode;
mod error;
mod value;

pub use decode::decode;
pub use encode::{encode, encode_into};
pub use error::Error;
pub use value::Value;

/// Maximum nesting depth accepted by [`decode`].
///
/// Bencode is recursive and a hostile input could otherwise drive the parser
/// into unbounded recursion. Real torrent and resume files nest a handful of
/// levels; this bound is far above anything legitimate.
pub const MAX_DEPTH: usize = 128;
