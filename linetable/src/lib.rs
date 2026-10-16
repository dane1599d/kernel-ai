//! Compact address-to-source table shared by the host tool and the kernel.
//!
//! `cargo xtask` builds the table from the kernel's DWARF data with [`Builder`] and
//! hands it to the kernel as a Limine module. The kernel reads it with
//! [`LineTable::parse`] and [`LineTable::resolve`] to show source lines on faults.
//!
//! Layout, all little-endian:
//! - header (24 bytes): magic `KAILINE1`, line count u32, symbol count u32,
//!   string bytes u32, reserved u32
//! - line entries (24 bytes each, sorted by address): start u64, length u32,
//!   file string offset u32, line u32, reserved u32
//! - symbol entries (24 bytes each, sorted by address): start u64, size u64,
//!   name string offset u32, reserved u32
//! - strings: NUL-terminated UTF-8

#![no_std]

#[cfg(any(feature = "alloc", test))]
extern crate alloc;

pub const MAGIC: [u8; 8] = *b"KAILINE1";
const HEADER_LEN: usize = 24;
const ENTRY_LEN: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    TooShort,
    BadMagic,
    Truncated,
}

/// What an address resolves to. Either part may be missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location<'a> {
    pub symbol: Option<&'a str>,
    pub file: Option<&'a str>,
    pub line: Option<u32>,
}

/// A parsed, borrowed view of a table.
#[derive(Debug, Clone, Copy)]
pub struct LineTable<'a> {
    lines: &'a [u8],
    symbols: &'a [u8],
    strings: &'a [u8],
}

impl<'a> LineTable<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        if data.len() < HEADER_LEN {
            return Err(Error::TooShort);
        }
        if data[..8] != MAGIC {
            return Err(Error::BadMagic);
        }
        let line_count = read_u32(data, 8) as usize;
        let symbol_count = read_u32(data, 12) as usize;
        let string_len = read_u32(data, 16) as usize;
        let lines_end = line_count
            .checked_mul(ENTRY_LEN)
            .and_then(|n| n.checked_add(HEADER_LEN))
            .ok_or(Error::Truncated)?;
        let symbols_end = symbol_count
            .checked_mul(ENTRY_LEN)
            .and_then(|n| n.checked_add(lines_end))
            .ok_or(Error::Truncated)?;
        let strings_end = symbols_end
            .checked_add(string_len)
            .ok_or(Error::Truncated)?;
        if strings_end > data.len() {
            return Err(Error::Truncated);
        }
        Ok(Self {
            lines: &data[HEADER_LEN..lines_end],
            symbols: &data[lines_end..symbols_end],
            strings: &data[symbols_end..strings_end],
        })
    }

    pub fn line_count(&self) -> usize {
        self.lines.len() / ENTRY_LEN
    }

    pub fn symbol_count(&self) -> usize {
        self.symbols.len() / ENTRY_LEN
    }

    /// Finds the symbol and source line covering `addr`.
    pub fn resolve(&self, addr: u64) -> Option<Location<'a>> {
        let symbol = last_at_or_below(self.symbol_count(), addr, |i| {
            read_u64(self.symbols, i * ENTRY_LEN)
        })
        .and_then(|i| {
            let entry = i * ENTRY_LEN;
            let start = read_u64(self.symbols, entry);
            let size = read_u64(self.symbols, entry + 8);
            (addr < start.saturating_add(size))
                .then(|| string_at(self.strings, read_u32(self.symbols, entry + 16)))
                .flatten()
        });
        let line = last_at_or_below(self.line_count(), addr, |i| {
            read_u64(self.lines, i * ENTRY_LEN)
        })
        .and_then(|i| {
            let entry = i * ENTRY_LEN;
            let start = read_u64(self.lines, entry);
            let len = u64::from(read_u32(self.lines, entry + 8));
            (addr < start.saturating_add(len.max(1))).then(|| {
                let file = string_at(self.strings, read_u32(self.lines, entry + 12));
                (file, read_u32(self.lines, entry + 16))
            })
        });
        if symbol.is_none() && line.is_none() {
            return None;
        }
        Some(Location {
            symbol,
            file: line.and_then(|(file, _)| file),
            line: line.map(|(_, line)| line),
        })
    }
}

/// Index of the last entry whose start address is at or below `addr`.
fn last_at_or_below(count: usize, addr: u64, start_of: impl Fn(usize) -> u64) -> Option<usize> {
    let (mut low, mut high) = (0, count);
    while low < high {
        let mid = low + (high - low) / 2;
        if start_of(mid) <= addr {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low.checked_sub(1)
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    let mut raw = [0; 4];
    raw.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(raw)
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(raw)
}

fn string_at(strings: &[u8], offset: u32) -> Option<&str> {
    let tail = strings.get(offset as usize..)?;
    let end = tail.iter().position(|&b| b == 0)?;
    core::str::from_utf8(&tail[..end]).ok()
}

#[cfg(feature = "alloc")]
pub use builder::Builder;

#[cfg(feature = "alloc")]
mod builder {
    use super::{ENTRY_LEN, HEADER_LEN, MAGIC};
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Collects lines and symbols in any order and serializes a sorted table.
    #[derive(Default)]
    pub struct Builder {
        lines: Vec<(u64, u32, u32, u32)>,
        symbols: Vec<(u64, u64, u32)>,
        strings: Vec<u8>,
        offsets: BTreeMap<String, u32>,
    }

    impl Builder {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn add_line(&mut self, addr: u64, len: u32, file: &str, line: u32) {
            let file = self.intern(file);
            self.lines.push((addr, len, file, line));
        }

        pub fn add_symbol(&mut self, addr: u64, size: u64, name: &str) {
            let name = self.intern(name);
            self.symbols.push((addr, size, name));
        }

        fn intern(&mut self, text: &str) -> u32 {
            if let Some(&offset) = self.offsets.get(text) {
                return offset;
            }
            let offset = u32::try_from(self.strings.len()).expect("string table under 4 GiB");
            self.strings.extend_from_slice(text.as_bytes());
            self.strings.push(0);
            self.offsets.insert(String::from(text), offset);
            offset
        }

        pub fn finish(mut self) -> Vec<u8> {
            self.lines.sort_by_key(|entry| entry.0);
            self.lines.dedup_by_key(|entry| entry.0);
            self.lines = merge_adjacent(core::mem::take(&mut self.lines));
            self.symbols.sort_by_key(|entry| entry.0);
            self.symbols.dedup_by_key(|entry| entry.0);
            let total = HEADER_LEN
                + (self.lines.len() + self.symbols.len()) * ENTRY_LEN
                + self.strings.len();
            let mut out = Vec::with_capacity(total);
            out.extend_from_slice(&MAGIC);
            out.extend_from_slice(&count(self.lines.len()).to_le_bytes());
            out.extend_from_slice(&count(self.symbols.len()).to_le_bytes());
            out.extend_from_slice(&count(self.strings.len()).to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            for (addr, len, file, line) in &self.lines {
                out.extend_from_slice(&addr.to_le_bytes());
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(&file.to_le_bytes());
                out.extend_from_slice(&line.to_le_bytes());
                out.extend_from_slice(&0u32.to_le_bytes());
            }
            for (addr, size, name) in &self.symbols {
                out.extend_from_slice(&addr.to_le_bytes());
                out.extend_from_slice(&size.to_le_bytes());
                out.extend_from_slice(&name.to_le_bytes());
                out.extend_from_slice(&0u32.to_le_bytes());
            }
            out.extend_from_slice(&self.strings);
            out
        }
    }

    fn count(n: usize) -> u32 {
        u32::try_from(n).expect("table sections under 4 GiB")
    }

    /// Joins sorted rows that touch and share a file and line into one row.
    fn merge_adjacent(rows: Vec<(u64, u32, u32, u32)>) -> Vec<(u64, u32, u32, u32)> {
        let mut merged: Vec<(u64, u32, u32, u32)> = Vec::with_capacity(rows.len());
        for row in rows {
            if let Some(last) = merged.last_mut() {
                let touches = last.0.saturating_add(u64::from(last.1)) == row.0;
                let combined = last.1.checked_add(row.1);
                if let (true, true, Some(len)) =
                    (touches, last.2 == row.2 && last.3 == row.3, combined)
                {
                    last.1 = len;
                    continue;
                }
            }
            merged.push(row);
        }
        merged
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn sample() -> Vec<u8> {
        let mut builder = Builder::new();
        builder.add_line(0x2010, 8, "kernel/src/b.rs", 7);
        builder.add_line(0x1000, 16, "kernel/src/a.rs", 3);
        builder.add_line(0x1010, 16, "kernel/src/a.rs", 4);
        builder.add_symbol(0x2000, 0x40, "kernel::b");
        builder.add_symbol(0x1000, 0x20, "kernel::a");
        builder.finish()
    }

    #[test]
    fn resolves_symbol_and_line_after_unsorted_input() {
        let data = sample();
        let table = LineTable::parse(&data).unwrap();
        assert_eq!(table.line_count(), 3);
        assert_eq!(table.symbol_count(), 2);
        let location = table.resolve(0x1014).unwrap();
        assert_eq!(location.symbol, Some("kernel::a"));
        assert_eq!(location.file, Some("kernel/src/a.rs"));
        assert_eq!(location.line, Some(4));
    }

    #[test]
    fn merges_touching_rows_with_same_location() {
        let mut builder = Builder::new();
        builder.add_line(0x1000, 4, "a.rs", 1);
        builder.add_line(0x1004, 4, "a.rs", 1);
        builder.add_line(0x1008, 4, "a.rs", 2);
        builder.add_line(0x1010, 4, "a.rs", 2);
        let data = builder.finish();
        let table = LineTable::parse(&data).unwrap();
        assert_eq!(table.line_count(), 3);
        assert_eq!(table.resolve(0x1006).unwrap().line, Some(1));
        assert_eq!(table.resolve(0x100c), None);
        assert_eq!(table.resolve(0x1012).unwrap().line, Some(2));
    }

    #[test]
    fn symbol_without_line_still_resolves() {
        let data = sample();
        let table = LineTable::parse(&data).unwrap();
        let location = table.resolve(0x2030).unwrap();
        assert_eq!(location.symbol, Some("kernel::b"));
        assert_eq!(location.line, None);
    }

    #[test]
    fn address_outside_everything_is_none() {
        let data = sample();
        let table = LineTable::parse(&data).unwrap();
        assert_eq!(table.resolve(0x0fff), None);
        assert_eq!(table.resolve(0x9000), None);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(LineTable::parse(b"short").unwrap_err(), Error::TooShort);
        let mut data = sample();
        data[0] = b'X';
        assert_eq!(LineTable::parse(&data).unwrap_err(), Error::BadMagic);
        let data = sample();
        assert_eq!(
            LineTable::parse(&data[..data.len() - 1]).unwrap_err(),
            Error::Truncated
        );
    }
}
