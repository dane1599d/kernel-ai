//! `kai-bundle 1` (ADR 0008): the boot drive's one file of programs, line tables
//! and components, `boot/kai.bundle`, in place of one Limine module per file, so
//! adding a program never changes the boot config.
//!
//! Layout, all integers little-endian:
//! - a 16-byte header: magic `KAIBNDL1`, version (u16, 1), entry count (u16), 4
//!   reserved bytes, zero;
//! - `count` entries of [`ENTRY_BYTES`]: the path (at most [`MAX_PATH`] bytes of
//!   UTF-8, zero-padded), the data's offset and length (u64 each) and its SHA-256;
//! - the data, each entry's at a 4 KiB boundary.
//!
//! [`parse`] checks every field before it trusts it and every entry's SHA-256
//! (against corruption, not an attacker); [`build`] (feature `alloc`) writes one.

#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod sha256;

pub const MAGIC: &[u8; 8] = b"KAIBNDL1";
pub const VERSION: u16 = 1;
pub const HEADER_BYTES: usize = 16;
pub const MAX_PATH: usize = 64;
pub const ENTRY_BYTES: usize = MAX_PATH + 8 + 8 + 32;
/// Where each entry's data starts.
pub const ALIGN: usize = 4096;
/// Entries a bundle may hold.
pub const MAX_ENTRIES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Shorter than its header or its entry table.
    Truncated,
    BadMagic,
    UnknownVersion(u16),
    TooManyEntries,
    /// An entry's path is empty, not UTF-8, or has a zero byte inside it.
    BadPath,
    /// An entry's data lies outside the bundle or off a 4 KiB boundary.
    BadRange,
    /// Two entries have the same path.
    DuplicatePath,
    /// An entry's data does not match its SHA-256.
    Corrupt,
}

/// One file in a bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    pub path: &'a str,
    pub data: &'a [u8],
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

/// A checked bundle.
#[derive(Clone, Copy, Debug)]
pub struct Bundle<'a> {
    bytes: &'a [u8],
    count: usize,
}

impl<'a> Bundle<'a> {
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The entries, in table order.
    pub fn entries(&self) -> impl Iterator<Item = Entry<'a>> + Clone + 'a {
        let bytes = self.bytes;
        (0..self.count).map(move |i| {
            entry(bytes, i).unwrap_or(Entry {
                path: "",
                data: &[],
            })
        })
    }

    pub fn get(&self, path: &str) -> Option<&'a [u8]> {
        self.entries().find(|e| e.path == path).map(|e| e.data)
    }
}

/// Entry `index` of `bytes`, with its path and range checked (not its digest).
fn entry(bytes: &[u8], index: usize) -> Result<Entry<'_>, Error> {
    let at = HEADER_BYTES + index * ENTRY_BYTES;
    let raw = bytes.get(at..at + ENTRY_BYTES).ok_or(Error::Truncated)?;
    let name = &raw[..MAX_PATH];
    let end = name.iter().position(|b| *b == 0).unwrap_or(MAX_PATH);
    if end == 0 || name[end..].iter().any(|b| *b != 0) {
        return Err(Error::BadPath);
    }
    let path = core::str::from_utf8(&name[..end]).map_err(|_| Error::BadPath)?;
    let (offset, length) = (u64_at(raw, MAX_PATH), u64_at(raw, MAX_PATH + 8));
    let start = usize::try_from(offset).map_err(|_| Error::BadRange)?;
    let len = usize::try_from(length).map_err(|_| Error::BadRange)?;
    let table_end = HEADER_BYTES + ENTRY_BYTES * u16_at(bytes, 10) as usize;
    if !start.is_multiple_of(ALIGN) || start < table_end {
        return Err(Error::BadRange);
    }
    let data = start
        .checked_add(len)
        .and_then(|end| bytes.get(start..end))
        .ok_or(Error::BadRange)?;
    Ok(Entry { path, data })
}

/// One table entry, read without the data: for a bundle on a disk, which the
/// reader streams entry by entry (the update disk, ADR 0008 §12) and checks
/// against the digest itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableEntry<'a> {
    pub path: &'a str,
    pub offset: u64,
    pub length: u64,
    pub sha256: [u8; 32],
}

/// The bytes of header and table a bundle whose first [`HEADER_BYTES`] are
/// `header` has: what a streaming reader reads next.
pub fn table_bytes(header: &[u8]) -> Result<usize, Error> {
    let header = header.get(..HEADER_BYTES).ok_or(Error::Truncated)?;
    if &header[..8] != MAGIC {
        return Err(Error::BadMagic);
    }
    let version = u16_at(header, 8);
    if version != VERSION {
        return Err(Error::UnknownVersion(version));
    }
    let count = u16_at(header, 10) as usize;
    if count > MAX_ENTRIES {
        return Err(Error::TooManyEntries);
    }
    Ok(HEADER_BYTES + count * ENTRY_BYTES)
}

/// Entry `index` of a bundle of `total` bytes whose header and table are
/// `table` (as [`table_bytes`] sized it): its path checked, and its data inside
/// the bundle and on a 4 KiB boundary. The digest is the caller's to check.
pub fn table_entry(table: &[u8], index: usize, total: u64) -> Result<TableEntry<'_>, Error> {
    let size = table_bytes(table)?;
    if table.len() < size || index >= (size - HEADER_BYTES) / ENTRY_BYTES {
        return Err(Error::Truncated);
    }
    let at = HEADER_BYTES + index * ENTRY_BYTES;
    let raw = &table[at..at + ENTRY_BYTES];
    let name = &raw[..MAX_PATH];
    let end = name.iter().position(|b| *b == 0).unwrap_or(MAX_PATH);
    if end == 0 || name[end..].iter().any(|b| *b != 0) {
        return Err(Error::BadPath);
    }
    let path = core::str::from_utf8(&name[..end]).map_err(|_| Error::BadPath)?;
    let (offset, length) = (u64_at(raw, MAX_PATH), u64_at(raw, MAX_PATH + 8));
    let inside = offset.checked_add(length).is_some_and(|end| end <= total);
    if !offset.is_multiple_of(ALIGN as u64) || offset < size as u64 || !inside {
        return Err(Error::BadRange);
    }
    let mut sha256 = [0u8; 32];
    sha256.copy_from_slice(&raw[MAX_PATH + 16..]);
    Ok(TableEntry {
        path,
        offset,
        length,
        sha256,
    })
}

/// Checks `bytes` as a `kai-bundle 1`: header, table, every path and range, no
/// path twice, and every entry's SHA-256.
pub fn parse(bytes: &[u8]) -> Result<Bundle<'_>, Error> {
    let header = bytes.get(..HEADER_BYTES).ok_or(Error::Truncated)?;
    if &header[..8] != MAGIC {
        return Err(Error::BadMagic);
    }
    let version = u16_at(header, 8);
    if version != VERSION {
        return Err(Error::UnknownVersion(version));
    }
    let count = u16_at(header, 10) as usize;
    if count > MAX_ENTRIES {
        return Err(Error::TooManyEntries);
    }
    if bytes.len() < HEADER_BYTES + count * ENTRY_BYTES {
        return Err(Error::Truncated);
    }
    for index in 0..count {
        let this = entry(bytes, index)?;
        if (0..index).any(|earlier| entry(bytes, earlier).is_ok_and(|e| e.path == this.path)) {
            return Err(Error::DuplicatePath);
        }
        let at = HEADER_BYTES + index * ENTRY_BYTES + MAX_PATH + 16;
        if sha256::digest(this.data) != bytes[at..at + 32] {
            return Err(Error::Corrupt);
        }
    }
    Ok(Bundle { bytes, count })
}

/// A `kai-bundle 1` holding `files` in order. Fails on a path that is empty,
/// too long or has a zero byte, a path given twice, or too many files.
#[cfg(feature = "alloc")]
pub fn build(files: &[(&str, &[u8])]) -> Result<alloc::vec::Vec<u8>, Error> {
    use alloc::vec::Vec;
    if files.len() > MAX_ENTRIES {
        return Err(Error::TooManyEntries);
    }
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    let mut offset = (HEADER_BYTES + files.len() * ENTRY_BYTES).next_multiple_of(ALIGN);
    for (i, (path, data)) in files.iter().enumerate() {
        let name = path.as_bytes();
        if name.is_empty() || name.len() > MAX_PATH || name.contains(&0) {
            return Err(Error::BadPath);
        }
        if files[..i].iter().any(|(other, _)| other == path) {
            return Err(Error::DuplicatePath);
        }
        let mut field = [0u8; MAX_PATH];
        field[..name.len()].copy_from_slice(name);
        out.extend_from_slice(&field);
        out.extend_from_slice(&(offset as u64).to_le_bytes());
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        out.extend_from_slice(&sha256::digest(data));
        offset = (offset + data.len()).next_multiple_of(ALIGN);
    }
    for (_, data) in files {
        out.resize(out.len().next_multiple_of(ALIGN), 0);
        out.extend_from_slice(data);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
