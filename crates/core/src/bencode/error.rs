//! Decoding errors.

use thiserror::Error;

/// A bencode decoding failure.
///
/// Every variant carries the byte offset at which the problem was found, so a
/// failure can be located precisely in the offending file.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum Error {
    /// Input ended in the middle of a value.
    #[error("unexpected end of input at byte {at}")]
    UnexpectedEof {
        /// Byte offset at which more input was required.
        at: usize,
    },

    /// A byte appeared where a value was expected.
    #[error("unexpected byte {byte:#04x} at byte {at}, expected a bencode value")]
    UnexpectedByte {
        /// Byte offset of the offending byte.
        at: usize,
        /// The byte that was found.
        byte: u8,
    },

    /// An integer was malformed or written non-canonically.
    #[error("malformed integer at byte {at}: {reason}")]
    InvalidInteger {
        /// Byte offset at which the integer began.
        at: usize,
        /// Why the integer was rejected.
        reason: &'static str,
    },

    /// An integer was canonical but does not fit in an [`i64`].
    #[error("integer at byte {at} does not fit in i64")]
    IntegerOverflow {
        /// Byte offset at which the integer began.
        at: usize,
    },

    /// A byte string length prefix was malformed or written non-canonically.
    #[error("malformed byte string length at byte {at}: {reason}")]
    InvalidLength {
        /// Byte offset at which the length prefix began.
        at: usize,
        /// Why the length was rejected.
        reason: &'static str,
    },

    /// A dictionary key was not a byte string.
    #[error("dictionary key at byte {at} is not a byte string")]
    NonByteStringKey {
        /// Byte offset of the offending key.
        at: usize,
    },

    /// Dictionary keys were not in strictly ascending byte order.
    #[error("dictionary key at byte {at} is out of order (keys must ascend)")]
    UnsortedDictKey {
        /// Byte offset of the offending key.
        at: usize,
    },

    /// The same dictionary key appeared twice.
    #[error("duplicate dictionary key at byte {at}")]
    DuplicateDictKey {
        /// Byte offset of the repeated key.
        at: usize,
    },

    /// Nesting exceeded [`super::MAX_DEPTH`].
    #[error("nesting deeper than {limit} at byte {at}")]
    DepthLimitExceeded {
        /// Byte offset at which the limit was hit.
        at: usize,
        /// The configured limit.
        limit: usize,
    },

    /// Bytes remained after a complete top-level value.
    #[error("{remaining} trailing byte(s) after the top-level value at byte {at}")]
    TrailingData {
        /// Byte offset of the first unconsumed byte.
        at: usize,
        /// How many bytes were left over.
        remaining: usize,
    },
}
