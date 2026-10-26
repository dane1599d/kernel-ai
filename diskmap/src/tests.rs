//! The writer's tables read back, and hostile tables are refused, never trusted.

extern crate std;

use super::*;
use alloc::vec;

struct Mem(Vec<u8>);

impl Disk for Mem {
    fn sectors(&self) -> u64 {
        (self.0.len() / SECTOR) as u64
    }
    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), Error> {
        let at = (lba as usize).checked_mul(SECTOR).ok_or(Error::Read)?;
        let src = self.0.get(at..at + out.len()).ok_or(Error::Read)?;
        out.copy_from_slice(src);
        Ok(())
    }
}

const SECTORS: u64 = 64 * 2048; // 64 MiB

fn g(n: u8) -> Guid {
    Guid([n; 16])
}

fn specs() -> Vec<Spec<'static>> {
    let data_first = align(FIRST_USABLE);
    let cache_first = 48 * 2048;
    vec![
        Spec {
            kind: KAI_DATA_TYPE,
            guid: g(1),
            first: data_first,
            last: cache_first - 1,
            name: "kai-data",
        },
        Spec {
            kind: KAI_CACHE_TYPE,
            guid: g(2),
            first: cache_first,
            last: last_usable(SECTORS),
            name: "kai-cache",
        },
    ]
}

fn disk(parts: &[Spec<'_>]) -> Mem {
    let mut bytes = vec![0u8; SECTORS as usize * SECTOR];
    for (lba, data) in layout(SECTORS, g(9), parts).unwrap() {
        let at = lba as usize * SECTOR;
        bytes[at..at + data.len()].copy_from_slice(&data);
    }
    Mem(bytes)
}

#[test]
fn a_written_table_reads_back() {
    let table = read(&mut disk(&specs())).unwrap();
    assert_eq!(table.disk_guid, g(9));
    assert!(!table.from_backup);
    assert_eq!(table.partitions.len(), 2);
    let data = table.find(KAI_DATA_TYPE).unwrap();
    assert_eq!((data.first, data.name.as_str()), (2048, "kai-data"));
    let cache = table.find(KAI_CACHE_TYPE).unwrap();
    assert_eq!(cache.last, last_usable(SECTORS));
    assert_eq!(cache.sectors(), SECTORS - 34 - 48 * 2048 + 1);
    assert!(table.find(ESP_TYPE).is_none());
}

#[test]
fn crc32_matches_the_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn a_bad_primary_falls_back_to_the_backup() {
    let mut d = disk(&specs());
    d.0[SECTOR + 40] ^= 1; // the primary header's first usable LBA: its CRC fails
    let table = read(&mut d).unwrap();
    assert!(table.from_backup);
    assert_eq!(table.partitions.len(), 2);
    // A bad primary array falls back too.
    let mut d = disk(&specs());
    d.0[2 * SECTOR + 33] ^= 1;
    assert!(read(&mut d).unwrap().from_backup);
    // Both bad: no table.
    let mut d = disk(&specs());
    d.0[SECTOR] = b'X';
    let last = (SECTORS as usize - 1) * SECTOR;
    d.0[last] = b'X';
    assert_eq!(read(&mut d), Err(Error::NoTable));
}

/// Rewrites the primary header with `edit` applied and a fresh CRC, and kills the
/// backup, so only the edited header is read.
fn edited(edit: impl Fn(&mut [u8])) -> Result<Table, Error> {
    let mut d = disk(&specs());
    let h = &mut d.0[SECTOR..2 * SECTOR];
    edit(h);
    h[16..20].fill(0);
    let crc = crc32(&h[..92]);
    h[16..20].copy_from_slice(&crc.to_le_bytes());
    let last = (SECTORS as usize - 1) * SECTOR;
    d.0[last] = b'X';
    read(&mut d)
}

#[test]
fn hostile_headers_are_refused() {
    let put32 =
        |at: usize, v: u32| move |h: &mut [u8]| h[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let put64 =
        |at: usize, v: u64| move |h: &mut [u8]| h[at..at + 8].copy_from_slice(&v.to_le_bytes());
    assert!(edited(|_| {}).is_ok());
    for bad in [
        edited(put32(80, 0)),            // no entries
        edited(put32(80, u32::MAX)),     // an array far too long
        edited(put32(84, 100)),          // an entry size not a power of two
        edited(put32(84, 1 << 20)),      // a huge entry size
        edited(put32(12, 600)),          // a header longer than a sector
        edited(put32(12, 10)),           // shorter than the fields
        edited(put64(24, 5)),            // says it lives elsewhere
        edited(put64(40, 0)),            // usable range over the header
        edited(put64(48, SECTORS)),      // usable range past the disk
        edited(put64(48, 10)),           // last usable before first
        edited(put64(72, SECTORS - 1)),  // array over the backup header
        edited(put64(72, u64::MAX - 1)), // array LBA overflows
        edited(put64(72, 2048)),         // array inside the usable range
    ] {
        assert!(bad.is_err(), "{bad:?}");
    }
}

#[test]
fn hostile_partitions_are_refused() {
    let mut overlap = specs();
    overlap[1].first = overlap[0].last;
    assert_eq!(layout(SECTORS, g(9), &overlap), Err(Error::BadLayout));
    // Written past the writer's checks: overlapping entries, and one outside the
    // usable range, each with a valid array CRC.
    for edit in [
        |e: &mut [u8]| e[128 + 32..128 + 40].copy_from_slice(&3000u64.to_le_bytes()),
        |e: &mut [u8]| e[40..48].copy_from_slice(&(SECTORS - 1).to_le_bytes()),
        |e: &mut [u8]| e[32..40].copy_from_slice(&1u64.to_le_bytes()),
        |e: &mut [u8]| {
            e[32..40].copy_from_slice(&5000u64.to_le_bytes());
            e[40..48].copy_from_slice(&4000u64.to_le_bytes());
        },
    ] {
        let mut d = disk(&specs());
        let array_at = 2 * SECTOR;
        let array_len = (ENTRIES * ENTRY_BYTES) as usize;
        edit(&mut d.0[array_at..array_at + array_len]);
        let crc = crc32(&d.0[array_at..array_at + array_len]);
        let h = &mut d.0[SECTOR..2 * SECTOR];
        h[88..92].copy_from_slice(&crc.to_le_bytes());
        h[16..20].fill(0);
        let hcrc = crc32(&h[..92]);
        h[16..20].copy_from_slice(&hcrc.to_le_bytes());
        let last = (SECTORS as usize - 1) * SECTOR;
        d.0[last] = b'X';
        assert_eq!(read(&mut d), Err(Error::BadPartitions));
    }
}

#[test]
fn layouts_that_do_not_fit_are_refused() {
    let mut past = specs();
    past[1].last = SECTORS;
    assert_eq!(layout(SECTORS, g(9), &past), Err(Error::BadLayout));
    let mut early = specs();
    early[0].first = 1;
    assert_eq!(layout(SECTORS, g(9), &early), Err(Error::BadLayout));
    let mut long = specs();
    long[0].name = "a name far longer than thirty-six UTF-16 units";
    assert_eq!(layout(SECTORS, g(9), &long), Err(Error::BadLayout));
    assert_eq!(layout(10, g(9), &[]), Err(Error::BadLayout));
    assert_eq!(read(&mut Mem(vec![0; 10 * SECTOR])), Err(Error::Small));
}

#[test]
fn random_bytes_never_panic() {
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for round in 0..2000 {
        let mut d = disk(&specs());
        // Flip bytes in the headers and arrays; fix the CRCs half the time, so
        // the checks behind them are reached.
        for _ in 0..1 + next() % 8 {
            let at = match next() % 3 {
                0 => SECTOR + (next() % 92) as usize,
                1 => 2 * SECTOR + (next() % (ENTRIES * ENTRY_BYTES) as u64) as usize,
                _ => (SECTORS as usize - 1) * SECTOR + (next() % 92) as usize,
            };
            d.0[at] = next() as u8;
        }
        if round % 2 == 0 {
            let array_len = (ENTRIES * ENTRY_BYTES) as usize;
            let crc = crc32(&d.0[2 * SECTOR..2 * SECTOR + array_len]);
            let h = &mut d.0[SECTOR..2 * SECTOR];
            h[88..92].copy_from_slice(&crc.to_le_bytes());
            h[16..20].fill(0);
            let size = (u32::from_le_bytes([h[12], h[13], h[14], h[15]]) as usize).clamp(92, 512);
            let hcrc = crc32(&h[..size]);
            h[16..20].copy_from_slice(&hcrc.to_le_bytes());
        }
        if let Ok(table) = read(&mut d) {
            for p in &table.partitions {
                assert!(p.first <= p.last && p.last < SECTORS);
            }
        }
    }
}
