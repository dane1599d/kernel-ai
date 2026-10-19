//! Bounds-checked little-endian reads over a byte slice.

use crate::Error;

pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    /// The bytes read since `start`, an earlier position of this cursor.
    pub fn since(&self, start: usize) -> &'a [u8] {
        &self.bytes[start..self.pos]
    }

    pub fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        if len > self.remaining() {
            return Err(Error::Truncated);
        }
        let out = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    /// A u64 length, then that many bytes of UTF-8.
    pub fn string(&mut self) -> Result<&'a str, Error> {
        let len = self.u64()?;
        let len = match usize::try_from(len) {
            Ok(len) if len <= self.remaining() => len,
            _ => return Err(Error::StringPastEnd),
        };
        core::str::from_utf8(self.take(len)?).map_err(|_| Error::BadUtf8)
    }
}
