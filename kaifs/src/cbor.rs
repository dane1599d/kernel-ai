//! The little CBOR (RFC 8949) kaifs writes itself: unsigned integers, text,
//! arrays and maps, all of definite length. `info.cbor` is its one user in
//! format 1. The reader takes exactly those, skips what it does not need, and
//! refuses the rest (floats, tags, byte strings, indefinite lengths), so a
//! hostile item cannot make it allocate more than the item's own length.

use crate::Error;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const UINT: u8 = 0;
const TEXT: u8 = 3;
const ARRAY: u8 = 4;
const MAP: u8 = 5;
/// How deep a value may nest.
const MAX_DEPTH: usize = 8;

fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    let m = major << 5;
    match n {
        0..=23 => out.push(m | n as u8),
        24..=0xFF => out.extend_from_slice(&[m | 24, n as u8]),
        0x100..=0xFFFF => {
            out.push(m | 25);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        0x1_0000..=0xFFFF_FFFF => {
            out.push(m | 26);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
        _ => {
            out.push(m | 27);
            out.extend_from_slice(&n.to_be_bytes());
        }
    }
}

pub fn uint(out: &mut Vec<u8>, n: u64) {
    head(out, UINT, n);
}

pub fn text(out: &mut Vec<u8>, s: &str) {
    head(out, TEXT, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

pub fn array(out: &mut Vec<u8>, len: u64) {
    head(out, ARRAY, len);
}

pub fn map(out: &mut Vec<u8>, len: u64) {
    head(out, MAP, len);
}

/// A reader over one CBOR item's bytes.
pub struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Reader { rest: bytes }
    }

    pub fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.rest.len() < n {
            return Err(Error::Corrupt("cbor"));
        }
        let (head, tail) = self.rest.split_at(n);
        self.rest = tail;
        Ok(head)
    }

    /// A head: its major type and argument.
    fn head(&mut self) -> Result<(u8, u64), Error> {
        let byte = self.take(1)?[0];
        let (major, info) = (byte >> 5, byte & 0x1F);
        let n = match info {
            0..=23 => u64::from(info),
            24 => u64::from(self.take(1)?[0]),
            25 => u64::from(u16::from_be_bytes([self.take(1)?[0], self.take(1)?[0]])),
            26 => {
                let mut b = [0; 4];
                b.copy_from_slice(self.take(4)?);
                u64::from(u32::from_be_bytes(b))
            }
            27 => {
                let mut b = [0; 8];
                b.copy_from_slice(self.take(8)?);
                u64::from_be_bytes(b)
            }
            _ => return Err(Error::Corrupt("cbor length")),
        };
        Ok((major, n))
    }

    fn expect(&mut self, major: u8) -> Result<u64, Error> {
        match self.head()? {
            (m, n) if m == major => Ok(n),
            _ => Err(Error::Corrupt("cbor type")),
        }
    }

    pub fn uint(&mut self) -> Result<u64, Error> {
        self.expect(UINT)
    }

    pub fn text(&mut self) -> Result<String, Error> {
        let len = usize::try_from(self.expect(TEXT)?).map_err(|_| Error::Corrupt("cbor length"))?;
        let bytes = self.take(len)?;
        core::str::from_utf8(bytes)
            .map(ToString::to_string)
            .map_err(|_| Error::Corrupt("cbor text"))
    }

    pub fn array(&mut self) -> Result<u64, Error> {
        self.expect(ARRAY)
    }

    pub fn map(&mut self) -> Result<u64, Error> {
        self.expect(MAP)
    }

    /// Skips one value of the kinds this reader knows.
    pub fn skip(&mut self) -> Result<(), Error> {
        self.skip_at(0)
    }

    fn skip_at(&mut self, depth: usize) -> Result<(), Error> {
        if depth > MAX_DEPTH {
            return Err(Error::Corrupt("cbor nesting"));
        }
        let (major, n) = self.head()?;
        match major {
            UINT => Ok(()),
            TEXT => {
                let len = usize::try_from(n).map_err(|_| Error::Corrupt("cbor length"))?;
                self.take(len).map(|_| ())
            }
            ARRAY | MAP => {
                let items = if major == MAP { n.saturating_mul(2) } else { n };
                // Each value takes a byte at least: a count past what is left is a lie.
                if items > self.rest.len() as u64 {
                    return Err(Error::Corrupt("cbor length"));
                }
                for _ in 0..items {
                    self.skip_at(depth + 1)?;
                }
                Ok(())
            }
            _ => Err(Error::Corrupt("cbor type")),
        }
    }
}
