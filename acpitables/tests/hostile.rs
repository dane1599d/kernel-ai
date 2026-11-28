//! Hostile firmware tables: cut memory, every field set to an extreme value, and
//! thousands of mutated tables whose checksums are fixed up again, so the
//! mutations reach the MADT, HPET, MCFG and FADT readers. Parsing returns a platform
//! or an error; it never panics, never reads past what memory returned, and
//! allocates no more than a small multiple of the tables.

#[path = "hostile/harness.rs"]
mod harness;
#[path = "../../testkit/hostile.rs"]
mod hostile;
#[path = "hostile/ivrs.rs"]
mod ivrs_harness;

use acpitables::{Error, MAX_TABLE_LEN, PhysMem, Platform, parse};
use hostile::XorShift;

const SDT_HEADER_LEN: usize = 36;
const ALLOC_PER_BYTE: usize = 4;
const ALLOC_FLOOR: usize = 4096;

/// Where the fake machine keeps its tables.
const XSDT_AT: usize = 0x100;
const TABLES_AT: [usize; 4] = [0x1000, 0x2000, 0x3000, 0x4000];
const MEM_LEN: usize = 0x5000;
/// FADT offsets (ACPI 6.5 table 5.9): IAPC_BOOT_ARCH, flags, the reset
/// register and its value.
const FADT_ARCH: usize = 109;
const FADT_FLAGS: usize = 112;
const FADT_RESET: usize = 116;
const FADT_RESET_VALUE: usize = 128;

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

/// QEMU q35's FADT, as far as it is read: revision 5, an 8042, and the reset
/// register at I/O port 0xCF9 taking 0x0F.
fn fadt() -> Vec<u8> {
    let mut b = vec![0u8; 276 - SDT_HEADER_LEN];
    let body = |at: usize| at - SDT_HEADER_LEN;
    b[body(FADT_ARCH)..body(FADT_ARCH) + 2].copy_from_slice(&2u16.to_le_bytes());
    b[body(FADT_FLAGS)..body(FADT_FLAGS) + 4].copy_from_slice(&(1u32 << 10).to_le_bytes());
    b[body(FADT_RESET)] = 1;
    b[body(FADT_RESET) + 1] = 8;
    b[body(FADT_RESET) + 4..body(FADT_RESET) + 12].copy_from_slice(&0xCF9u64.to_le_bytes());
    b[body(FADT_RESET_VALUE)] = 0x0F;
    let mut t = sdt(b"FACP", &b);
    t[8] = 5;
    t
}

/// RSDP v2 at 0 and an XSDT pointing at the tables, each table's checksum
/// fixed over the length its own header claims.
fn machine(tables: &[Vec<u8>]) -> Vec<u8> {
    let mut mem = vec![0u8; MEM_LEN];
    let mut pointers = Vec::new();
    for &at in &TABLES_AT[..tables.len()] {
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
    let ends = [madt().len(), hpet().len()];
    hostile::truncations(&mem, |cut, prefix| {
        let result = check(&flat(prefix.to_vec()));
        // The MADT must be whole; an HPET whose signature is cut off is skipped,
        // and one cut after its signature is unreadable. An MCFG cut anywhere is
        // left out, as if absent.
        let whole = |i: usize| cut >= TABLES_AT[i] + ends[i];
        let skipped = |i: usize| cut < TABLES_AT[i] + 4;
        let expected = whole(0) && (whole(1) || skipped(1));
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

fn with_fadt(fadt: Vec<u8>) -> Vec<u8> {
    let [madt, hpet, mcfg] = tables();
    machine(&[madt, hpet, mcfg, fadt])
}

#[test]
fn the_fadt_sample_parses() {
    let platform = check(&flat(with_fadt(fadt()))).unwrap();
    let reset = platform.reset.expect("q35's reset register");
    assert_eq!((reset.register.address, reset.value), (0xCF9, 0x0F));
    assert!(!platform.no_8042);
}

#[test]
fn every_cut_of_the_fadt_with_its_length_and_checksum_fixed() {
    let sample = fadt();
    hostile::truncations(&sample, |cut, prefix| {
        let mut table = prefix.to_vec();
        if table.len() >= 8 {
            table[4..8].copy_from_slice(&(cut as u32).to_le_bytes());
        }
        let _ = check(&flat(with_fadt(table)));
    });
}

#[test]
fn every_field_of_the_fadt_with_a_good_checksum() {
    let sample = fadt();
    hostile::sweep_fields(&sample, 0..sample.len(), |_, table| {
        let _ = check(&flat(with_fadt(table.to_vec())));
    });
}

#[test]
fn mutated_fadts_with_good_checksums() {
    let sample = fadt();
    let mut rng = XorShift::new(0xFAD7_0007);
    let mut parsed = 0;
    for _ in 0..4000 {
        let table = hostile::mutate(&mut rng, &sample);
        if let Ok(platform) = check(&flat(with_fadt(table))) {
            parsed += 1;
            // A reset register is kept only with an address.
            assert!(platform.reset.is_none_or(|r| r.register.address != 0));
        }
    }
    assert!(parsed > 1000, "{parsed} of 4000 parsed");
}

/// Bug hunt 3 H3-4: a damaged FADT or MCFG is left out, as if absent, and the
/// other tables are kept. A damaged MADT or HPET still stops the parse.
#[test]
fn a_damaged_fadt_or_mcfg_is_left_out() {
    let bad = Error::BadChecksum("system description table");
    // Bodies changed after their checksums were set: the FADT, then the MCFG.
    let mut mem = with_fadt(fadt());
    mem[TABLES_AT[3] + 40] ^= 0xFF;
    let platform = check(&flat(mem)).unwrap();
    assert_eq!(platform.cpus, [0, 1]);
    assert_eq!(platform.hpet, Some(0xFED0_0000));
    assert_eq!(platform.ecam.len(), 2);
    assert_eq!((platform.reset, platform.no_8042), (None, false));
    assert_eq!(platform.ignored, [("FACP", bad)]);
    let mut mem = machine(&tables());
    mem[TABLES_AT[2] + 50] ^= 0xFF;
    let platform = check(&flat(mem)).unwrap();
    assert_eq!(platform.cpus, [0, 1]);
    assert_eq!(platform.hpet, Some(0xFED0_0000));
    assert!(platform.ecam.is_empty());
    assert_eq!(platform.ignored, [("MCFG", bad)]);
    // An MCFG longer than the limit, and a FADT shorter than a header.
    let mut long_mcfg = mcfg();
    long_mcfg[4..8].copy_from_slice(&(MAX_TABLE_LEN as u32 + 1).to_le_bytes());
    let mut short_fadt = fadt();
    short_fadt[4..8].copy_from_slice(&20u32.to_le_bytes());
    let damaged = [madt(), hpet(), long_mcfg, short_fadt];
    let platform = check(&flat(machine(&damaged))).unwrap();
    let short = Error::BadChecksum("table length");
    let ignored = [("MCFG", Error::TableTooLong), ("FACP", short)];
    assert_eq!(platform.ignored, ignored);
    for which in [0, 1] {
        let mut mem = machine(&tables());
        mem[TABLES_AT[which] + 40] ^= 0xFF;
        assert_eq!(check(&flat(mem)), Err(bad), "table {which}");
    }
}

// G34: the fuzz target's body (`harness.rs`), firmware memory as one window
// at a fixed RSDP that refuses and counts every read outside it.

/// The sample machine moved into the window: every address in it shifted by
/// [`harness::BASE`], checksums fixed again.
fn windowed(mem: &[u8]) -> Vec<u8> {
    let mut mem = mem.to_vec();
    let base = harness::BASE;
    let shift = |mem: &mut Vec<u8>, at: usize, width: usize| {
        let mut word = [0u8; 8];
        word[..width].copy_from_slice(&mem[at..at + width]);
        let moved = (u64::from_le_bytes(word) + base).to_le_bytes();
        mem[at..at + width].copy_from_slice(&moved[..width]);
    };
    shift(&mut mem, 16, 4);
    shift(&mut mem, 24, 8);
    harness::fix(&mut mem[..20], 8);
    harness::fix(&mut mem[..36], 32);
    let len = u32::from_le_bytes(mem[XSDT_AT + 4..XSDT_AT + 8].try_into().unwrap()) as usize;
    for at in (XSDT_AT + SDT_HEADER_LEN..XSDT_AT + len).step_by(8) {
        shift(&mut mem, at, 8);
    }
    harness::fix(&mut mem[XSDT_AT..XSDT_AT + len], 9);
    mem
}

#[test]
fn the_windowed_sample_parses_with_no_read_refused() {
    let mem = windowed(&with_fadt(fadt()));
    let window = harness::Window::new(&mem);
    let platform = parse(&window, harness::BASE).unwrap();
    assert_eq!(platform.cpus, [0, 1]);
    assert!(platform.reset.is_some());
    assert_eq!(window.refused.get(), 0);
    harness::acpi_tables(&mem);
    // Unshifted, every table pointer lands below the window and is refused:
    // no MADT, and nothing read outside.
    let raw = with_fadt(fadt());
    let window = harness::Window::new(&raw);
    assert!(parse(&window, harness::BASE).is_err());
}

#[test]
fn random_windows_never_read_outside() {
    let mut rng = XorShift::new(0xac91);
    for _ in 0..3_000 {
        let len = rng.below(512);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        harness::acpi_tables(&bytes);
    }
}

#[test]
fn mutated_windows_never_read_outside() {
    let mem = windowed(&with_fadt(fadt()));
    // The RSDP and the root table, then each table's header and start.
    let mut ranges = vec![0..36, XSDT_AT..XSDT_AT + 68];
    ranges.extend(TABLES_AT.iter().map(|&at| at..at + 48));
    for (i, range) in ranges.into_iter().enumerate() {
        hostile::fuzz(&mem[range.clone()], 0xac00 + i as u64, 500, |part| {
            let mut whole = mem.clone();
            let n = part.len().min(range.len());
            whole[range.start..range.start + n].copy_from_slice(&part[..n]);
            harness::acpi_tables(&whole);
        });
        hostile::sweep_fields(&mem, range, |_, mutated| harness::acpi_tables(mutated));
    }
}

#[test]
fn pointers_past_the_window_are_refused_and_counted() {
    let mem = windowed(&with_fadt(fadt()));
    for address in [
        0,
        harness::BASE - 1,
        harness::BASE + MEM_LEN as u64 - 3,
        u64::MAX - 1,
    ] {
        let mut mem = mem.clone();
        // The HPET's pointer, the second in the root table.
        let at = XSDT_AT + SDT_HEADER_LEN + 8;
        mem[at..at + 8].copy_from_slice(&address.to_le_bytes());
        harness::fix(&mut mem[XSDT_AT..XSDT_AT + SDT_HEADER_LEN + 32], 9);
        let window = harness::Window::new(&mem);
        let platform = parse(&window, harness::BASE).expect("an unreadable pointer is skipped");
        assert_eq!(platform.hpet, None, "{address:#x}");
        assert_eq!(window.refused.get(), 1, "{address:#x}");
    }
}

#[test]
fn refixed_tables_let_mutations_reach_the_readers() {
    let mem = windowed(&with_fadt(fadt()));
    let mut rng = XorShift::new(0xac5e);
    let (mut parsed, mut fadt_read) = (0, 0);
    for _ in 0..2_000 {
        let mut whole = mem.clone();
        // A mutation inside one listed table's body, past its header.
        let at = TABLES_AT[rng.below(4)] + SDT_HEADER_LEN + rng.below(40);
        whole[at] = rng.next() as u8;
        // Its checksum is now wrong, as a fuzzer's mutation leaves it; the
        // harness's re-fix mends the root and every listed table.
        harness::refix_tables(&mut whole);
        let platform = harness::check(&whole);
        parsed += usize::from(platform.is_some());
        fadt_read += usize::from(platform.is_some_and(|p| p.ignored.is_empty()));
    }
    assert_eq!(parsed, 2_000, "every re-fixed mutation parses");
    assert!(fadt_read > 1_900, "{fadt_read} with no table left out");
}

/// Writes the `acpi_tables` and `ivrs_tables` targets' seeds, one file each,
/// to `$FUZZ_SEED_DIR/<target>/`: the sample machine as a window, and QEMU's
/// own IVRS tables.
#[test]
#[ignore = "writes the fuzz corpus; run by the fuzz workflow"]
fn write_fuzz_seeds() {
    let dir = std::env::var("FUZZ_SEED_DIR").expect("FUZZ_SEED_DIR names the corpus folder");
    let write = |target: &str, seeds: &[(&str, &[u8])]| {
        let folder = std::path::Path::new(&dir).join(target);
        std::fs::create_dir_all(&folder).unwrap();
        for (name, bytes) in seeds {
            std::fs::write(folder.join(name), bytes).unwrap();
        }
    };
    write("acpi_tables", &[("machine", &windowed(&with_fadt(fadt())))]);
    write(
        "ivrs_tables",
        &[
            ("q35", &include_bytes!("fixtures/ivrs/q35.bin")[..]),
            (
                "q35-devices",
                &include_bytes!("fixtures/ivrs/q35-devices.bin")[..],
            ),
            (
                "q35-xtsup",
                &include_bytes!("fixtures/ivrs/q35-xtsup.bin")[..],
            ),
        ],
    );
}

#[test]
fn the_fuzz_targets_body_reads_the_machine_and_the_captured_tables() {
    harness::acpi_tables(&windowed(&with_fadt(fadt())));
    for table in [
        &include_bytes!("fixtures/ivrs/q35.bin")[..],
        &include_bytes!("fixtures/ivrs/q35-devices.bin")[..],
        &include_bytes!("fixtures/ivrs/q35-xtsup.bin")[..],
    ] {
        ivrs_harness::ivrs_tables(table);
        harness::acpi_tables(table);
    }
}
