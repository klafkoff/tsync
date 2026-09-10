//! Strict, canonical-only bencode decoding.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use super::{Error, MAX_DEPTH, Value};

/// Decodes exactly one bencode value from `input`.
///
/// The whole of `input` must be a single value; leftover bytes are an error
/// rather than a silently ignored tail.
///
/// Non-canonical encodings are rejected rather than normalized, which is what
/// makes `encode(decode(bytes)) == bytes` hold for every accepted input. See
/// the [module documentation](super) for the reasoning and the full list of
/// rejected forms.
///
/// # Errors
///
/// Returns [`Error`] if the input is truncated, malformed, non-canonical,
/// carries trailing bytes, or nests deeper than [`MAX_DEPTH`].
pub fn decode(input: &[u8]) -> Result<Value, Error> {
    let mut decoder = Decoder { input, pos: 0 };
    let value = decoder.value(0)?;
    if decoder.pos != input.len() {
        return Err(Error::TrailingData {
            at: decoder.pos,
            remaining: input.len() - decoder.pos,
        });
    }
    Ok(value)
}

struct Decoder<'a> {
    input: &'a [u8],
    pos: usize,
}

impl Decoder<'_> {
    fn peek(&self) -> Result<u8, Error> {
        self.input
            .get(self.pos)
            .copied()
            .ok_or(Error::UnexpectedEof { at: self.pos })
    }

    /// Offset of the next `needle` at or after the cursor.
    fn find(&self, needle: u8) -> Result<usize, Error> {
        self.input[self.pos..]
            .iter()
            .position(|&byte| byte == needle)
            .map(|offset| self.pos + offset)
            .ok_or(Error::UnexpectedEof {
                at: self.input.len(),
            })
    }

    fn value(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_DEPTH {
            return Err(Error::DepthLimitExceeded {
                at: self.pos,
                limit: MAX_DEPTH,
            });
        }
        match self.peek()? {
            b'i' => self.integer().map(Value::Integer),
            b'l' => self.list(depth),
            b'd' => self.dict(depth),
            b'0'..=b'9' => self.byte_string().map(Value::Bytes),
            byte => Err(Error::UnexpectedByte { at: self.pos, byte }),
        }
    }

    fn integer(&mut self) -> Result<i64, Error> {
        let at = self.pos;
        self.pos += 1; // consume 'i'
        let end = self.find(b'e')?;
        let body = &self.input[self.pos..end];
        self.pos = end + 1;

        let digits = match body.first() {
            None => {
                return Err(Error::InvalidInteger {
                    at,
                    reason: "empty",
                });
            }
            Some(b'-') => {
                let rest = &body[1..];
                if rest == b"0" {
                    // i-0e and i0e would both decode to 0, so only one can be
                    // canonical. Bencode says i0e.
                    return Err(Error::InvalidInteger {
                        at,
                        reason: "negative zero",
                    });
                }
                rest
            }
            Some(_) => body,
        };

        if digits.is_empty() {
            return Err(Error::InvalidInteger {
                at,
                reason: "missing digits",
            });
        }
        if !digits.iter().all(u8::is_ascii_digit) {
            return Err(Error::InvalidInteger {
                at,
                reason: "non-digit character",
            });
        }
        if digits.len() > 1 && digits[0] == b'0' {
            return Err(Error::InvalidInteger {
                at,
                reason: "leading zero",
            });
        }

        let text = core::str::from_utf8(body).map_err(|_| Error::InvalidInteger {
            at,
            reason: "not ascii",
        })?;
        text.parse::<i64>()
            .map_err(|_| Error::IntegerOverflow { at })
    }

    fn byte_string(&mut self) -> Result<Vec<u8>, Error> {
        let at = self.pos;
        let colon = self.find(b':')?;
        let digits = &self.input[self.pos..colon];

        if digits.is_empty() {
            return Err(Error::InvalidLength {
                at,
                reason: "empty",
            });
        }
        if !digits.iter().all(u8::is_ascii_digit) {
            return Err(Error::InvalidLength {
                at,
                reason: "non-digit character",
            });
        }
        if digits.len() > 1 && digits[0] == b'0' {
            return Err(Error::InvalidLength {
                at,
                reason: "leading zero",
            });
        }

        let text = core::str::from_utf8(digits).map_err(|_| Error::InvalidLength {
            at,
            reason: "not ascii",
        })?;
        let length: usize = text.parse().map_err(|_| Error::InvalidLength {
            at,
            reason: "length does not fit in usize",
        })?;

        let start = colon + 1;
        let end = start.checked_add(length).ok_or(Error::InvalidLength {
            at,
            reason: "length overflows the address space",
        })?;
        if end > self.input.len() {
            return Err(Error::UnexpectedEof {
                at: self.input.len(),
            });
        }

        self.pos = end;
        Ok(self.input[start..end].to_vec())
    }

    fn list(&mut self, depth: usize) -> Result<Value, Error> {
        self.pos += 1; // consume 'l'
        let mut items = Vec::new();
        while self.peek()? != b'e' {
            items.push(self.value(depth + 1)?);
        }
        self.pos += 1; // consume 'e'
        Ok(Value::List(items))
    }

    fn dict(&mut self, depth: usize) -> Result<Value, Error> {
        self.pos += 1; // consume 'd'
        let mut map: BTreeMap<Vec<u8>, Value> = BTreeMap::new();

        while self.peek()? != b'e' {
            let key_at = self.pos;
            if !self.peek()?.is_ascii_digit() {
                return Err(Error::NonByteStringKey { at: key_at });
            }
            let key = self.byte_string()?;

            // The map is sorted, so the greatest key inserted so far is the
            // only one a canonical stream could be following.
            if let Some((previous, _)) = map.last_key_value() {
                match key.cmp(previous) {
                    Ordering::Less => return Err(Error::UnsortedDictKey { at: key_at }),
                    Ordering::Equal => return Err(Error::DuplicateDictKey { at: key_at }),
                    Ordering::Greater => {}
                }
            }

            let value = self.value(depth + 1)?;
            map.insert(key, value);
        }

        self.pos += 1; // consume 'e'
        Ok(Value::Dict(map))
    }
}
