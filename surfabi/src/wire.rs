//! The wire: Wayland's message layout in little-endian words. A message is an
//! 8-byte header, the object id and then `size << 16 | opcode`, followed by
//! 32-bit arguments. A string is its length with the NUL, then the bytes, the NUL
//! and padding to 4; an array is its length, then the bytes and padding. `size`
//! counts the header and is a multiple of 4, at most [`MAX_MESSAGE`].
//!
//! Wayland allows a null string (length 0); no request or event here takes one,
//! so length 0 is an error.

use crate::MAX_MESSAGE;
use alloc::vec::Vec;

/// The header's length.
pub const HEADER: usize = 8;

/// Why a message was refused. Any of these closes the connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Fewer bytes than the header or the size says.
    Short,
    /// A size under the header, not a multiple of 4, or over [`MAX_MESSAGE`].
    BadSize,
    /// An argument runs past the message's end.
    Truncated,
    /// A string that is empty, has no NUL at its end, a NUL inside, or is not
    /// UTF-8.
    BadString,
    /// Bytes left after the last argument.
    Trailing,
    /// No object has this id on this connection.
    UnknownObject,
    /// The object's interface has no such opcode.
    UnknownOpcode,
    /// An argument out of its range: object id 0, a negative size, an unknown
    /// enum value.
    BadArg,
}

/// A message's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub object: u32,
    pub opcode: u16,
    /// Bytes in the message, header included.
    pub size: u16,
}

/// The first message in `buf`: its header, its argument bytes and what follows
/// it. `Short` when `buf` does not hold all of it yet.
pub fn split(buf: &[u8]) -> Result<(Header, &[u8], &[u8]), Error> {
    if buf.len() < HEADER {
        return Err(Error::Short);
    }
    let object = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let word = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let header = Header {
        object,
        opcode: (word & 0xFFFF) as u16,
        size: (word >> 16) as u16,
    };
    let size = usize::from(header.size);
    if size < HEADER || !size.is_multiple_of(4) || size > MAX_MESSAGE {
        return Err(Error::BadSize);
    }
    if buf.len() < size {
        return Err(Error::Short);
    }
    Ok((header, &buf[HEADER..size], &buf[size..]))
}

/// A 24.8 fixed-point number, as Wayland's `wl_fixed_t`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fixed(pub i32);

impl Fixed {
    pub const fn from_int(n: i32) -> Fixed {
        Fixed(n.wrapping_mul(256))
    }

    pub const fn trunc(self) -> i32 {
        self.0 >> 8
    }
}

/// Reads one message's arguments in order. Each read checks its bounds, and
/// [`Reader::end`] checks nothing is left over.
pub struct Reader<'a> {
    args: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(args: &'a [u8]) -> Self {
        Self { args }
    }

    fn word(&mut self) -> Result<u32, Error> {
        let (word, rest) = self.args.split_first_chunk::<4>().ok_or(Error::Truncated)?;
        self.args = rest;
        Ok(u32::from_le_bytes(*word))
    }

    pub fn uint(&mut self) -> Result<u32, Error> {
        self.word()
    }

    pub fn int(&mut self) -> Result<i32, Error> {
        self.word().map(|w| w as i32)
    }

    /// An int that must be 0 or more: a size, an offset or a position in a
    /// buffer.
    pub fn size(&mut self) -> Result<u32, Error> {
        u32::try_from(self.int()?).map_err(|_| Error::BadArg)
    }

    pub fn fixed(&mut self) -> Result<Fixed, Error> {
        self.int().map(Fixed)
    }

    /// A new object's id, never 0.
    pub fn new_id(&mut self) -> Result<u32, Error> {
        self.object()
    }

    /// An object's id, never 0 (no argument here may be null).
    pub fn object(&mut self) -> Result<u32, Error> {
        match self.word()? {
            0 => Err(Error::BadArg),
            id => Ok(id),
        }
    }

    pub fn array(&mut self) -> Result<&'a [u8], Error> {
        let len = self.word()? as usize;
        let padded = len.checked_add(3).ok_or(Error::Truncated)? & !3;
        if padded > self.args.len() {
            return Err(Error::Truncated);
        }
        let (bytes, rest) = self.args.split_at(padded);
        self.args = rest;
        Ok(&bytes[..len])
    }

    pub fn string(&mut self) -> Result<&'a str, Error> {
        let bytes = self.array()?;
        let (&last, text) = bytes.split_last().ok_or(Error::BadString)?;
        if last != 0 || text.contains(&0) {
            return Err(Error::BadString);
        }
        core::str::from_utf8(text).map_err(|_| Error::BadString)
    }

    /// Ok when every argument was read.
    pub fn end(self) -> Result<(), Error> {
        match self.args.is_empty() {
            true => Ok(()),
            false => Err(Error::Trailing),
        }
    }
}

/// Builds one message. [`Writer::finish`] fills in the size, and refuses a
/// message over [`MAX_MESSAGE`].
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub fn new(object: u32, opcode: u16) -> Self {
        let mut bytes = Vec::with_capacity(64);
        bytes.extend_from_slice(&object.to_le_bytes());
        bytes.extend_from_slice(&u32::from(opcode).to_le_bytes());
        Self { bytes }
    }

    pub fn uint(mut self, value: u32) -> Self {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn int(self, value: i32) -> Self {
        self.uint(value as u32)
    }

    pub fn fixed(self, value: Fixed) -> Self {
        self.int(value.0)
    }

    pub fn array(mut self, value: &[u8]) -> Self {
        self.bytes
            .extend_from_slice(&(value.len() as u32).to_le_bytes());
        self.bytes.extend_from_slice(value);
        self.pad()
    }

    /// `value` with its NUL. The caller passes no NUL inside it; [`Reader`]
    /// refuses one.
    pub fn string(mut self, value: &str) -> Self {
        let len = value.len() + 1;
        self.bytes.extend_from_slice(&(len as u32).to_le_bytes());
        self.bytes.extend_from_slice(value.as_bytes());
        self.bytes.push(0);
        self.pad()
    }

    fn pad(mut self) -> Self {
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
        self
    }

    pub fn finish(mut self) -> Result<Vec<u8>, Error> {
        let size = self.bytes.len();
        if size > MAX_MESSAGE {
            return Err(Error::BadSize);
        }
        let word = (size as u32) << 16 | u32::from_le_bytes([self.bytes[4], self.bytes[5], 0, 0]);
        self.bytes[4..8].copy_from_slice(&word.to_le_bytes());
        Ok(self.bytes)
    }
}
