//! Hostile firmware tables: cut memory, every field set to an extreme value, and
//! thousands of mutated tables whose checksums are fixed up again, so the
//! mutations reach the MADT, HPET and MCFG readers. Parsing returns a platform
//! or an error; it never panics, never reads past what memory returned, and
//! allocates no more than a small multiple of the tables.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use acpitables::{Error, MAX_TABLE_LEN, PhysMem, Platform, parse};
use hostile::XorShift;

const SDT_HEADER_LEN: usize = 36;
const ALLOC_PER_BYTE: usize = 4;
const ALLOC_FLOOR: usize = 4096;

/// Where the fake machine keeps its tables.
const XSDT_AT: usize = 0x100;
const TABLES_AT: [usize; 3] = [0x1000, 0x2000, 0x3000];
const MEM_LEN: usize = 0x4000;

/// Fake physical memory from address 0, which may hand back fewer bytes than
/// asked for, as a buggy mapper might.
struct Flat {
    bytes: Vec<u8>,
    short: bool,
}

impl PhysMem for Flat {
    fn read(&self, phys: u64, len: usize) -> Option<&[u8]> {
        let start = usize::try_from(phys).ok()?;
        let end = start.checked_add(len)?;
        let bytes = self.bytes.get(start..end)?;
        Some(if self.short { &bytes[..len / 2] } else { bytes })
    }
}

fn flat(bytes: Vec<u8>) -> Flat {
    Flat {
        bytes,
        short: false,
    }
}

/// Sets byte 9 (a table's checksum) so the `len` bytes at `at` sum to zero, as
/// far as memory goes.
fn fix_checksum(mem: &mut [u8], at: usize, len: usize, checksum_at: usize) {
    let end = at.saturating_add(len).min(mem.len());
    if at + checksum_at >= end {
        return;
    }
    mem[at + checksum_at] = 0;
    let sum = mem[at..end].iter().fold(0u8, |s, &b| s.wrapping_add(b));
    mem[at + checksum_at] = 0u8.wrapping_sub(sum);
}

fn sdt(signature: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut t = vec![0u8; SDT_HEADER_LEN];
    t[..4].copy_from_slice(signature);
    t.extend_from_slice(body);
    let len = t.len() as u32;
    t[4..8].copy_from_slice(&len.to_le_bytes());
    t
}

fn madt() -> Vec<u8> {
    let mut b = vec![];
    b.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    b.extend_from_slice(&[0, 8, 0, 0, 1, 0, 0, 0]);
    b.extend_from_slice(&[0, 8, 1, 1, 1, 0, 0, 0]);
    b.extend_from_slice(&[1, 12, 7, 0]);
    b.extend_from_slice(&0xFEC0_0000u32.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&[2, 10, 0, 9]);
    b.extend_from_slice(&9u32.to_le_bytes());
    b.extend_from_slice(&0b1111u16.to_le_bytes());
    b.extend_from_slice(&[5, 12, 0, 0]);
    b.extend_from_slice(&0xFEE0_0000u64.to_le_bytes());
    sdt(b"APIC", &b)
}

fn hpet() -> Vec<u8> {
    let mut b = vec![0u8; 4];
    b.extend_from_slice(&[0, 64, 0, 0]);
    b.extend_from_slice(&0xFED0_0000u64.to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 0]);
    sdt(b"HPET", &b)
}

fn mcfg() -> Vec<u8> {
    let mut b = vec![0u8; 8];
    for base in [0xB000_0000u64, 0xC000_0000] {
        b.extend_from_slice(&base.to_le_bytes());
        b.extend_from_slice(&[0, 0, 0, 255, 0, 0, 0, 0]);
    }
    sdt(b"MCFG", &b)
}

/// RSDP v2 at 0 and an XSDT pointing at three tables, each table's checksum
/// fixed over the length its own header claims.
fn machine(tables: &[Vec<u8>; 3]) -> Vec<u8> {
    let mut mem = vec![0u8; MEM_LEN];
    let mut pointers = Vec::new();
    for at in TABLES_AT {
        pointers.extend_from_slice(&(at as u64).to_le_bytes());
    }
    let xsdt = sdt(b"XSDT", &pointers);
    mem[XSDT_AT..XSDT_AT + xsdt.len()].copy_from_slice(&xsdt);
    fix_checksum(&mut mem, XSDT_AT, xsdt.len(), 9);
    for (table, at) in tables.iter().zip(TABLES_AT) {
        // A mutated table may have grown; it is cut at the next one.
        let len = table.len().min(0x1000);
        mem[at..at + len].copy_from_slice(&table[..len]);
        let claimed = table
            .get(4..8)
            .map_or(len, |b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
        fix_checksum(&mut mem, at, claimed, 9);
    }
    mem[..8].copy_from_slice(b"RSD PTR ");
    mem[15] = 2;
    mem[20..24].copy_from_slice(&36u32.to_le_bytes());
    mem[24..32].copy_from_slice(&(XSDT_AT as u64).to_le_bytes());
    fix_checksum(&mut mem, 0, 20, 8);
    fix_checksum(&mut mem, 0, 36, 32);
    mem
}

fn tables() -> [Vec<u8>; 3] {
    [madt(), hpet(), mcfg()]
}

fn check(mem: &Flat) -> Result<Platform, Error> {
    let (result, peak) = hostile::peak_alloc(|| parse(mem, 0));
    let cap = ALLOC_PER_BYTE * mem.bytes.len() + ALLOC_FLOOR;
    assert!(
        peak <= cap,
        "{peak} bytes allocated for {} bytes",
        mem.bytes.len()
    );
    result
}

#[test]
fn the_sample_parses() {
    let platform = check(&flat(machine(&tables()))).unwrap();
    assert_eq!(platform.cpus, [0, 1]);
    assert_eq!(platform.io_apics.len(), 1);
    assert_eq!(platform.hpet, Some(0xFED0_0000));
    assert_eq!(platform.ecam.len(), 2);
}

#[test]
fn every_prefix_of_memory() {
    let mem = machine(&tables());
    let ends = [madt().len(), hpet().len(), mcfg().len()];
    hostile::truncations(&mem, |cut, prefix| {
        let result = check(&flat(prefix.to_vec()));
        // The MADT must be whole; a table whose signature is cut off is skipped,
        // and one cut after its signature is unreadable.
        let whole = |i: usize| cut >= TABLES_AT[i] + ends[i];
        let skipped = |i: usize| cut < TABLES_AT[i] + 4;
        let expected = whole(0) && (1..3).all(|i| whole(i) || skipped(i));
        assert_eq!(result.is_ok(), expected, "cut at {cut}: {result:?}");
    });
}

#[test]
fn every_field_of_memory_set_to_every_extreme() {
    let mem = machine(&tables());
    // The RSDP and the XSDT, then the start of each table.
    let mut ranges = vec![0..36, XSDT_AT..XSDT_AT + 60];
    ranges.extend(TABLES_AT.iter().map(|&at| at..at + 48));
    for range in ranges {
        hostile::sweep_fields(&mem, range, |_, mutated| {
            let _ = check(&flat(mutated.to_vec()));
        });
    }
}

#[test]
fn mutated_tables_with_good_checksums() {
    let sample = tables();
    let mut rng = XorShift::new(0x853c_49e6_748f_ea9b);
    let mut parsed = 0;
    for _ in 0..4000 {
        let mut tables = sample.clone();
        let which = rng.below(3);
        tables[which] = hostile::mutate(&mut rng, &sample[which]);
        parsed += usize::from(check(&flat(machine(&tables))).is_ok());
    }
    // The fixed checksums let most mutations through to the table readers.
    assert!(parsed > 1000, "{parsed} of 4000 parsed");
}

#[test]
fn every_field_of_each_table_with_good_checksums() {
    let sample = tables();
    for which in 0..3 {
        hostile::sweep_fields(&sample[which], 0..sample[which].len(), |_, table| {
            let mut tables = sample.clone();
            tables[which] = table.to_vec();
            let _ = check(&flat(machine(&tables)));
        });
    }
}

#[test]
fn short_tables_and_lengths() {
    // A MADT just its header, or too short for its fixed fields: no CPUs, no
    // panic.
    for len in [36, 37, 40, 43, 44, 45] {
        let mut madt = madt();
        madt.truncate(len);
        madt[4..8].copy_from_slice(&(len as u32).to_le_bytes());
        let platform = check(&flat(machine(&[madt, hpet(), mcfg()]))).unwrap();
        assert!(platform.cpus.is_empty(), "{len}");
        assert_eq!(platform.local_apic != 0, len >= 44, "{len}");
    }
    // An HPET too short for its address, and an MCFG too short for any entry.
    let mut short_hpet = hpet();
    short_hpet.truncate(51);
    short_hpet[4..8].copy_from_slice(&51u32.to_le_bytes());
    let mut short_mcfg = mcfg();
    short_mcfg.truncate(59);
    short_mcfg[4..8].copy_from_slice(&59u32.to_le_bytes());
    let platform = check(&flat(machine(&[madt(), short_hpet, short_mcfg]))).unwrap();
    assert_eq!((platform.hpet, platform.ecam.len()), (None, 0));
    // Lengths below a header, past memory, and past the limit.
    for (len, expected) in [
        (0, Error::BadChecksum("table length")),
        (35, Error::BadChecksum("table length")),
        (MEM_LEN as u32, Error::Unreadable),
        (MAX_TABLE_LEN as u32 + 1, Error::TableTooLong),
        (u32::MAX, Error::TableTooLong),
    ] {
        let mut madt = madt();
        madt[4..8].copy_from_slice(&len.to_le_bytes());
        assert_eq!(
            check(&flat(machine(&[madt, hpet(), mcfg()]))),
            Err(expected),
            "{len}"
        );
    }
    // MADT entries of length 0, 1 and past the end stop the walk.
    for entry in [[0u8, 0], [0, 1], [0, 0xFF]] {
        let mut madt = madt();
        madt.extend_from_slice(&entry);
        let len = madt.len() as u32;
        madt[4..8].copy_from_slice(&len.to_le_bytes());
        let platform = check(&flat(machine(&[madt, hpet(), mcfg()]))).unwrap();
        assert_eq!(platform.cpus, [0, 1]);
    }
}

#[test]
fn memory_that_hands_back_less_than_asked() {
    let mem = Flat {
        bytes: machine(&tables()),
        short: true,
    };
    assert_eq!(check(&mem), Err(Error::Unreadable));
}

#[test]
fn root_tables_pointing_anywhere() {
    for address in [
        0u64,
        XSDT_AT as u64,
        u64::MAX,
        u64::MAX - 3,
        MEM_LEN as u64 - 2,
    ] {
        let mut mem = machine(&tables());
        mem[XSDT_AT + 36..XSDT_AT + 44].copy_from_slice(&address.to_le_bytes());
        fix_checksum(&mut mem, XSDT_AT, 60, 9);
        let _ = check(&flat(mem));
    }
    // An RSDT (revision 0) over the XSDT's bytes, with a partial entry at the
    // end: the first four bytes of the first pointer are the MADT's address.
    let mut mem = machine(&tables());
    mem[15] = 0;
    mem[16..20].copy_from_slice(&(XSDT_AT as u32).to_le_bytes());
    mem[XSDT_AT + 4..XSDT_AT + 8].copy_from_slice(&43u32.to_le_bytes());
    fix_checksum(&mut mem, XSDT_AT, 43, 9);
    fix_checksum(&mut mem, 0, 20, 8);
    assert_eq!(check(&flat(mem)).map(|p| p.cpus), Ok(vec![0, 1]));
}
