//! GPT, read and written without the standard library (ADR 0004 §5, ADR 0008 §1):
//! the kernel finds `kai-data` and `kai-cache` by partition type GUID, and xtask
//! lays out disks with the same code. Grown from `xtask/src/image/gpt.rs`.
//!
//! The reader trusts nothing on the disk: every header and entry array is checked
//! (signature, sizes, CRC-32s, LBAs inside the disk, partitions inside the usable
//! range and not overlapping) before any of it is used, and a bad primary header
//! falls back to the backup at the disk's last sector.

#![no_std]

extern crate alloc;

#[cfg(test)]
mod tests;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub const SECTOR: usize = 512;

/// A GUID as GPT stores it: the first three fields little-endian.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Guid(pub [u8; 16]);

/// The EFI system partition's type, C12A7328-F81F-11D2-BA4B-00A0C93EC93B.
pub const ESP_TYPE: Guid = Guid([
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
]);
/// `kai-data`'s type, `gpt::guid("kai-data partition type")` (ADR 0008 §1):
/// ED4A5DB6-8A45-4B43-A62C-43439C248FAC.
pub const KAI_DATA_TYPE: Guid = Guid([
    0xB6, 0x5D, 0x4A, 0xED, 0x45, 0x8A, 0x43, 0x4B, 0xA6, 0x2C, 0x43, 0x43, 0x9C, 0x24, 0x8F, 0xAC,
]);
/// `kai-cache`'s type, `gpt::guid("kai-cache partition type")`: a rebuildable
/// cache that holds no user data (the model lane's KV cache, S10-3):
/// 6CE7E9CF-03EF-4788-A942-7709C27665C2.
pub const KAI_CACHE_TYPE: Guid = Guid([
    0xCF, 0xE9, 0xE7, 0x6C, 0xEF, 0x03, 0x88, 0x47, 0xA9, 0x42, 0x77, 0x09, 0xC2, 0x76, 0x65, 0xC2,
]);

/// The slots' types (ADR 0008 §1), `gpt::guid("kai-r partition type")` and so
/// on: recovery, then the two system slots.
pub const KAI_R_TYPE: Guid = Guid([
    0x8C, 0x26, 0xC2, 0xB9, 0xA5, 0x17, 0xAF, 0x4F, 0xBC, 0xCB, 0x93, 0xB2, 0xFB, 0x9B, 0x83, 0x46,
]);
pub const KAI_A_TYPE: Guid = Guid([
    0x80, 0xFC, 0x0E, 0x56, 0xF3, 0xCF, 0x87, 0x45, 0xB5, 0x63, 0x7B, 0xA5, 0xC7, 0xC1, 0x87, 0xCA,
]);
pub const KAI_B_TYPE: Guid = Guid([
    0xCF, 0xCE, 0x90, 0x7B, 0x49, 0x86, 0x2E, 0x45, 0xA7, 0xD4, 0x43, 0xF3, 0x8F, 0xAD, 0xD5, 0x61,
]);

/// Entries in the array this crate writes, and at most this many are read.
pub const ENTRIES: u32 = 128;
pub const ENTRY_BYTES: u32 = 128;
/// Sectors the written entry array takes.
pub const ENTRY_SECTORS: u64 = (ENTRIES * ENTRY_BYTES) as u64 / SECTOR as u64;
/// The first usable LBA of a disk this crate writes.
pub const FIRST_USABLE: u64 = 2 + ENTRY_SECTORS;
/// Longest entry array read, in bytes.
const MAX_ARRAY: u64 = 64 * 1024;
const HEADER_BYTES: u32 = 92;
/// UTF-16 units in a partition name.
const NAME_UNITS: usize = 36;

/// The IEEE CRC-32 GPT uses.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// One partition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition {
    pub kind: Guid,
    pub guid: Guid,
    /// First and last LBA, inclusive.
    pub first: u64,
    pub last: u64,
    pub name: String,
}

impl Partition {
    pub fn sectors(&self) -> u64 {
        self.last - self.first + 1
    }
}

/// A disk's partition table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub disk_guid: Guid,
    pub first_usable: u64,
    pub last_usable: u64,
    pub partitions: Vec<Partition>,
    /// The primary header or its array was bad and the backup was read.
    pub from_backup: bool,
}

impl Table {
    /// The first partition of type `kind`.
    pub fn find(&self, kind: Guid) -> Option<&Partition> {
        self.partitions.iter().find(|p| p.kind == kind)
    }
}

/// Why a disk's table cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The disk could not be read.
    Read,
    /// Too small to hold a GPT.
    Small,
    /// Neither header is a valid GPT header.
    NoTable,
    /// A header is valid but its partitions are not.
    BadPartitions,
    /// A layout to write does not fit the disk or overlaps itself.
    BadLayout,
}

/// Reads sectors of a disk.
pub trait Disk {
    /// Size in 512-byte sectors.
    fn sectors(&self) -> u64;
    /// Fills `out`, whole sectors, from `lba`.
    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), Error>;
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from(u32_at(b, at)) | (u64::from(u32_at(b, at + 4)) << 32)
}

fn guid_at(b: &[u8], at: usize) -> Guid {
    let mut g = [0u8; 16];
    g.copy_from_slice(&b[at..at + 16]);
    Guid(g)
}

/// A header's fields, checked against itself and the disk.
struct Header {
    first_usable: u64,
    last_usable: u64,
    disk_guid: Guid,
    entries_at: u64,
    entries: u32,
    entry_bytes: u32,
    entries_crc: u32,
}

fn header(sector: &[u8], at: u64, sectors: u64) -> Option<Header> {
    if &sector[..8] != b"EFI PART" || u32_at(sector, 8) != 0x0001_0000 {
        return None;
    }
    let size = u32_at(sector, 12);
    if !(HEADER_BYTES..=SECTOR as u32).contains(&size) {
        return None;
    }
    let mut copy = [0u8; SECTOR];
    copy[..size as usize].copy_from_slice(&sector[..size as usize]);
    copy[16..20].fill(0);
    if crc32(&copy[..size as usize]) != u32_at(sector, 16) || u64_at(sector, 24) != at {
        return None;
    }
    let h = Header {
        first_usable: u64_at(sector, 40),
        last_usable: u64_at(sector, 48),
        disk_guid: guid_at(sector, 56),
        entries_at: u64_at(sector, 72),
        entries: u32_at(sector, 80),
        entry_bytes: u32_at(sector, 84),
        entries_crc: u32_at(sector, 88),
    };
    let array = u64::from(h.entries) * u64::from(h.entry_bytes);
    let array_sectors = array.div_ceil(SECTOR as u64);
    let sane = h.entry_bytes >= 128
        && h.entry_bytes.is_power_of_two()
        && h.entry_bytes <= 4096
        && h.entries > 0
        && array <= MAX_ARRAY
        && h.first_usable >= 2
        && h.first_usable <= h.last_usable
        && h.last_usable < sectors - 1
        && h.entries_at >= 2
        && h.entries_at.checked_add(array_sectors).is_some_and(|end| end < sectors)
        // The array lies outside the usable range.
        && (h.entries_at + array_sectors <= h.first_usable || h.entries_at > h.last_usable);
    sane.then_some(h)
}

fn name(entry: &[u8]) -> String {
    let units = entry[56..56 + 2 * NAME_UNITS]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|u| u16::from_le_bytes(*u))
        .take_while(|&u| u != 0);
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn partitions(h: &Header, array: &[u8]) -> Result<Vec<Partition>, Error> {
    let mut out: Vec<Partition> = Vec::new();
    for entry in array.chunks_exact(h.entry_bytes as usize) {
        let kind = guid_at(entry, 0);
        if kind == Guid::default() {
            continue;
        }
        let (first, last) = (u64_at(entry, 32), u64_at(entry, 40));
        if first > last || first < h.first_usable || last > h.last_usable {
            return Err(Error::BadPartitions);
        }
        if out.iter().any(|p| first <= p.last && p.first <= last) {
            return Err(Error::BadPartitions);
        }
        out.push(Partition {
            kind,
            guid: guid_at(entry, 16),
            first,
            last,
            name: name(entry),
        });
    }
    Ok(out)
}

fn read_table(disk: &mut dyn Disk, at: u64, sectors: u64) -> Result<Table, Error> {
    let mut sector = [0u8; SECTOR];
    disk.read(at, &mut sector)?;
    let h = header(&sector, at, sectors).ok_or(Error::NoTable)?;
    let bytes = (u64::from(h.entries) * u64::from(h.entry_bytes)) as usize;
    let mut array = vec![0u8; bytes.next_multiple_of(SECTOR)];
    disk.read(h.entries_at, &mut array)?;
    array.truncate(bytes);
    if crc32(&array) != h.entries_crc {
        return Err(Error::NoTable);
    }
    Ok(Table {
        disk_guid: h.disk_guid,
        first_usable: h.first_usable,
        last_usable: h.last_usable,
        partitions: partitions(&h, &array)?,
        from_backup: at != 1,
    })
}

/// Reads a disk's partition table: the primary, or else the backup.
pub fn read(disk: &mut dyn Disk) -> Result<Table, Error> {
    let sectors = disk.sectors();
    if sectors < 2 * (1 + ENTRY_SECTORS) + 2 {
        return Err(Error::Small);
    }
    match read_table(disk, 1, sectors) {
        Ok(table) => Ok(table),
        Err(Error::Read) => Err(Error::Read),
        // With no backup either, say what was wrong with the primary.
        Err(primary) => read_table(disk, sectors - 1, sectors).map_err(|backup| match backup {
            Error::NoTable => primary,
            other => other,
        }),
    }
}

/// A partition to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec<'a> {
    pub kind: Guid,
    pub guid: Guid,
    pub first: u64,
    pub last: u64,
    pub name: &'a str,
}

/// The first LBA at or after `lba` on a 1 MiB boundary.
pub fn align(lba: u64) -> u64 {
    lba.next_multiple_of(2048)
}

/// The last usable LBA of a disk of `sectors` this crate writes.
pub fn last_usable(sectors: u64) -> u64 {
    sectors - 2 - ENTRY_SECTORS
}

/// The sectors a GPT of `parts` on a disk of `sectors` takes, as (LBA, bytes):
/// the protective MBR, the primary header and array, the backup array and header.
/// Nothing in between is touched.
pub fn layout(
    sectors: u64,
    disk_guid: Guid,
    parts: &[Spec<'_>],
) -> Result<Vec<(u64, Vec<u8>)>, Error> {
    if sectors < 2 * (1 + ENTRY_SECTORS) + 2 || parts.len() > ENTRIES as usize {
        return Err(Error::BadLayout);
    }
    let last = last_usable(sectors);
    let mut array = vec![0u8; (ENTRIES * ENTRY_BYTES) as usize];
    for (i, (part, entry)) in parts
        .iter()
        .zip(array.as_chunks_mut::<{ ENTRY_BYTES as usize }>().0)
        .enumerate()
    {
        let overlaps = parts[..i]
            .iter()
            .any(|p| part.first <= p.last && p.first <= part.last);
        if part.first > part.last
            || part.first < FIRST_USABLE
            || part.last > last
            || overlaps
            || part.kind == Guid::default()
            || part.name.encode_utf16().count() > NAME_UNITS
        {
            return Err(Error::BadLayout);
        }
        entry[..16].copy_from_slice(&part.kind.0);
        entry[16..32].copy_from_slice(&part.guid.0);
        entry[32..40].copy_from_slice(&part.first.to_le_bytes());
        entry[40..48].copy_from_slice(&part.last.to_le_bytes());
        for (j, unit) in part.name.encode_utf16().enumerate() {
            entry[56 + 2 * j..58 + 2 * j].copy_from_slice(&unit.to_le_bytes());
        }
    }
    let crc = crc32(&array);
    let backup_array = sectors - 1 - ENTRY_SECTORS;
    Ok(vec![
        (0, protective_mbr(sectors).to_vec()),
        (1, header_sector(sectors, true, crc, disk_guid).to_vec()),
        (2, array.clone()),
        (backup_array, array),
        (
            sectors - 1,
            header_sector(sectors, false, crc, disk_guid).to_vec(),
        ),
    ])
}

fn protective_mbr(sectors: u64) -> [u8; SECTOR] {
    let mut s = [0u8; SECTOR];
    let entry = &mut s[446..462];
    entry[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    entry[4] = 0xEE;
    entry[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    entry[8..12].copy_from_slice(&1u32.to_le_bytes());
    let size = u32::try_from(sectors - 1).unwrap_or(u32::MAX);
    entry[12..16].copy_from_slice(&size.to_le_bytes());
    s[510] = 0x55;
    s[511] = 0xAA;
    s
}

fn header_sector(sectors: u64, primary: bool, entries_crc: u32, disk: Guid) -> [u8; SECTOR] {
    let last = sectors - 1;
    let (mine, other, entries_at) = match primary {
        true => (1, last, 2),
        false => (last, 1, last - ENTRY_SECTORS),
    };
    let mut s = [0u8; SECTOR];
    s[..8].copy_from_slice(b"EFI PART");
    s[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    s[12..16].copy_from_slice(&HEADER_BYTES.to_le_bytes());
    s[24..32].copy_from_slice(&mine.to_le_bytes());
    s[32..40].copy_from_slice(&other.to_le_bytes());
    s[40..48].copy_from_slice(&FIRST_USABLE.to_le_bytes());
    s[48..56].copy_from_slice(&last_usable(sectors).to_le_bytes());
    s[56..72].copy_from_slice(&disk.0);
    s[72..80].copy_from_slice(&entries_at.to_le_bytes());
    s[80..84].copy_from_slice(&ENTRIES.to_le_bytes());
    s[84..88].copy_from_slice(&ENTRY_BYTES.to_le_bytes());
    s[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = crc32(&s[..HEADER_BYTES as usize]);
    s[16..20].copy_from_slice(&crc.to_le_bytes());
    s
}
