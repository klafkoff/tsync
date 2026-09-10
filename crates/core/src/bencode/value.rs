//! The decoded bencode value type.

use std::collections::BTreeMap;

/// A decoded bencode value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// A bencode integer, written `i<digits>e`.
    Integer(i64),

    /// A bencode byte string, written `<length>:<bytes>`.
    ///
    /// Bencode strings are arbitrary bytes, not text. Torrent metadata
    /// routinely carries filenames in unknown or mixed encodings, so this is
    /// deliberately [`Vec<u8>`] rather than [`String`]; converting to text is
    /// a decision for the caller to make explicitly, at the point where it
    /// knows what encoding to assume.
    Bytes(Vec<u8>),

    /// A bencode list, written `l<values>e`.
    List(Vec<Value>),

    /// A bencode dictionary, written `d<pairs>e`, keyed by byte string.
    ///
    /// Held in a [`BTreeMap`] so iteration is in ascending byte order, which
    /// is precisely bencode's canonical key ordering. Encoding is therefore
    /// canonical by construction rather than by a sort step that a future
    /// change could omit.
    Dict(BTreeMap<Vec<u8>, Value>),
}

impl Value {
    /// Returns the integer, or `None` if this is not a [`Value::Integer`].
    #[must_use]
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(n) => Some(*n),
            _ => None,
        }
    }

    /// Returns the byte string, or `None` if this is not a [`Value::Bytes`].
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// Returns the list items, or `None` if this is not a [`Value::List`].
    #[must_use]
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    /// Returns the dictionary, or `None` if this is not a [`Value::Dict`].
    #[must_use]
    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Value>> {
        match self {
            Self::Dict(map) => Some(map),
            _ => None,
        }
    }

    /// Returns a mutable reference to the dictionary, or `None` if this is not
    /// a [`Value::Dict`].
    ///
    /// This is the entry point for rewriting resume data in place: fetch the
    /// dictionary, replace the handful of path fields, and re-encode.
    pub fn as_dict_mut(&mut self) -> Option<&mut BTreeMap<Vec<u8>, Value>> {
        match self {
            Self::Dict(map) => Some(map),
            _ => None,
        }
    }

    /// Looks up `key` in a dictionary value.
    ///
    /// Returns `None` if this is not a dictionary or the key is absent.
    #[must_use]
    pub fn get(&self, key: &[u8]) -> Option<&Value> {
        self.as_dict()?.get(key)
    }

    /// A short name for this value's type, for use in error messages.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Integer(_) => "integer",
            Self::Bytes(_) => "byte string",
            Self::List(_) => "list",
            Self::Dict(_) => "dictionary",
        }
    }
}
