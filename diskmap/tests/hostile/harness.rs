//! The `gpt_read` fuzz target's body (plan P6 G33), which
//! `diskmap/tests/hostile.rs` also runs over hostile disks, the way
//! `fuzz/harness.rs` serves its targets: a later
//! `fuzz/fuzz_targets/gpt_read.rs` includes this file by `#[path]` and calls
//! [`gpt_read`] with libFuzzer's input. It must return for any input, and
//! only a bug in `diskmap` may panic.
//!
//! The input is a disk image, whole 512-byte sectors. What a table read from
//! it must keep, so A/B rollback and the installer are never misled: no read
//! past the disk's end (the fake disk refuses one and counts it), a bounded
//! number of reads, and partitions that lie inside the header's usable range,
//! inside the disk and clear of both headers, and that do not overlap.

#![allow(dead_code)]

use diskmap::{
    Disk, ESP_TYPE, Error, Guid, KAI_A_TYPE, KAI_B_TYPE, KAI_CACHE_TYPE, KAI_DATA_TYPE, KAI_R_TYPE,
    SECTOR, Table, crc32, read,
};

/// The types the kernel and the installer look a partition up by.
pub const KINDS: [Guid; 6] = [
    ESP_TYPE,
    KAI_DATA_TYPE,
    KAI_CACHE_TYPE,
    KAI_R_TYPE,
    KAI_A_TYPE,
    KAI_B_TYPE,
];

/// A disk of whole sectors that refuses, and counts, a read past its end.
pub struct Image<'a> {
    pub bytes: &'a [u8],
    pub reads: usize,
    pub refused: usize,
}

impl<'a> Image<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        let whole = bytes.len() / SECTOR * SECTOR;
        Self {
            bytes: &bytes[..whole],
            reads: 0,
            refused: 0,
        }
    }
}

impl Disk for Image<'_> {
    fn sectors(&self) -> u64 {
        (self.bytes.len() / SECTOR) as u64
    }

    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), Error> {
        self.reads += 1;
        assert!(
            out.len().is_multiple_of(SECTOR),
            "a read of part of a sector"
        );
        let src = usize::try_from(lba)
            .ok()
            .and_then(|lba| lba.checked_mul(SECTOR))
            .and_then(|at| Some(at..at.checked_add(out.len())?))
            .and_then(|range| self.bytes.get(range));
        match src {
            Some(src) => {
                out.copy_from_slice(src);
                Ok(())
            }
            None => {
                self.refused += 1;
                Err(Error::Read)
            }
        }
    }
}

/// Reads `data` as a disk, and again with every CRC rewritten, so a mutation
/// is checked past the CRCs, not only refused by them.
pub fn gpt_read(data: &[u8]) {
    check(data);
    let mut fixed = data[..data.len() / SECTOR * SECTOR].to_vec();
    if recrc(&mut fixed) {
        check(&fixed);
    }
}

/// The promises, for `data` if a table reads from it.
pub fn check(data: &[u8]) -> Option<Table> {
    let mut disk = Image::new(data);
    let sectors = disk.sectors();
    let result = read(&mut disk);
    // The reader checks every LBA against the disk before it reads it.
    assert_eq!(disk.refused, 0, "a read past the disk's end");
    // The primary header and array, then the backup's.
    assert!(disk.reads <= 4, "{} reads", disk.reads);
    let table = result.ok()?;
    assert!(table.first_usable >= 2 && table.first_usable <= table.last_usable);
    assert!(
        table.last_usable < sectors - 1,
        "usable over the backup header"
    );
    for (i, p) in table.partitions.iter().enumerate() {
        assert!(p.kind != Guid::default());
        assert!(table.first_usable <= p.first && p.first <= p.last);
        assert!(p.last <= table.last_usable && p.last < sectors);
        assert!(p.sectors() >= 1 && p.sectors() <= sectors);
        assert!(p.name.encode_utf16().count() <= 36);
        for q in &table.partitions[i + 1..] {
            assert!(p.last < q.first || q.last < p.first, "{p:?} over {q:?}");
        }
    }
    // A slot the table names is one of its partitions, inside the disk.
    for kind in KINDS {
        if let Some(p) = table.find(kind) {
            assert!(p.kind == kind && p.last < sectors);
        }
    }
    Some(table)
}

/// Rewrites, in place, the entry array CRC and the header CRC of each header
/// whose fields put both inside `disk`; whether any was rewritten.
pub fn recrc(disk: &mut [u8]) -> bool {
    let sectors = (disk.len() / SECTOR) as u64;
    if sectors < 3 {
        return false;
    }
    let mut any = false;
    for at in [1, sectors - 1] {
        let base = at as usize * SECTOR;
        let word =
            |d: &[u8], i: usize| u32::from_le_bytes(d[base + i..base + i + 4].try_into().unwrap());
        let size = word(disk, 12) as usize;
        if !(92..=SECTOR).contains(&size) {
            continue;
        }
        let entries_at = u64::from_le_bytes(disk[base + 72..base + 80].try_into().unwrap());
        let bytes = u64::from(word(disk, 80)) * u64::from(word(disk, 84));
        let array = usize::try_from(entries_at)
            .ok()
            .and_then(|lba| lba.checked_mul(SECTOR))
            .zip(usize::try_from(bytes).ok())
            .and_then(|(start, len)| Some(start..start.checked_add(len)?))
            .filter(|r| r.end <= disk.len() && r.len() <= 64 * 1024);
        if let Some(array) = array {
            let crc = crc32(&disk[array]);
            disk[base + 88..base + 92].copy_from_slice(&crc.to_le_bytes());
        }
        disk[base + 16..base + 20].fill(0);
        let crc = crc32(&disk[base..base + size]);
        disk[base + 16..base + 20].copy_from_slice(&crc.to_le_bytes());
        any = true;
    }
    any
}
