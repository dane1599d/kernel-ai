//! Hostile GPT disks (plan P6 G33): random sectors, every field of both
//! headers and of the entries at every extreme, random mutations, each read as
//! it is and with its CRCs rewritten so the mutation reaches the checks behind
//! them. Each reads into a table that keeps every promise A/B rollback and the
//! installer rely on (`hostile/harness.rs`), or fails with an error. None
//! panics, and none reads past the disk.

#[path = "hostile/harness.rs"]
mod harness;
#[path = "../../testkit/hostile.rs"]
mod hostile;

use diskmap::{
    FIRST_USABLE, Guid, KAI_A_TYPE, KAI_B_TYPE, KAI_DATA_TYPE, KAI_R_TYPE, SECTOR, Spec,
    last_usable, layout, read,
};
use hostile::{XorShift, fuzz, sweep_fields};

/// A small disk: 128 sectors, room for four small partitions.
const SECTORS: u64 = 128;

fn g(n: u8) -> Guid {
    Guid([0x40 + n; 16])
}

/// A disk with R, A, B and data, as `layout` writes it.
fn sample() -> Vec<u8> {
    let first = FIRST_USABLE;
    let last = last_usable(SECTORS);
    let parts = [
        (KAI_R_TYPE, first, first + 9, "kai-r"),
        (KAI_A_TYPE, first + 10, first + 19, "kai-a"),
        (KAI_B_TYPE, first + 20, first + 29, "kai-b"),
        (KAI_DATA_TYPE, first + 30, last, "kai-data"),
    ];
    let specs: Vec<Spec> = parts
        .iter()
        .enumerate()
        .map(|(i, &(kind, first, last, name))| Spec {
            kind,
            guid: g(i as u8 + 1),
            first,
            last,
            name,
        })
        .collect();
    let mut disk = vec![0u8; SECTORS as usize * SECTOR];
    for (lba, bytes) in layout(SECTORS, g(0), &specs).unwrap() {
        let at = lba as usize * SECTOR;
        disk[at..at + bytes.len()].copy_from_slice(&bytes);
    }
    disk
}

const PRIMARY: usize = SECTOR;
const BACKUP: usize = (SECTORS as usize - 1) * SECTOR;
/// The primary array's first four entries, where the sample's partitions are.
const ENTRIES: std::ops::Range<usize> = 2 * SECTOR..2 * SECTOR + 4 * 128;

#[test]
fn the_sample_reads_with_its_slots() {
    let table = harness::check(&sample()).expect("the sample reads");
    assert!(!table.from_backup);
    assert_eq!(table.partitions.len(), 4);
    assert!(table.find(KAI_A_TYPE).is_some() && table.find(KAI_B_TYPE).is_some());
}

#[test]
fn a_bad_primary_falls_back_to_the_backup_and_both_bad_reads_nothing() {
    let mut disk = sample();
    disk[PRIMARY + 40] ^= 1;
    let table = harness::check(&disk).expect("the backup reads");
    assert!(table.from_backup);
    assert_eq!(table, {
        let mut t = harness::check(&sample()).unwrap();
        t.from_backup = true;
        t
    });
    disk[BACKUP + 40] ^= 1;
    assert!(harness::check(&disk).is_none());
}

#[test]
fn random_sectors_never_panic_or_read_past_the_disk() {
    let mut rng = XorShift::new(0x6971);
    for _ in 0..300 {
        let sectors = rng.below(2 * SECTORS as usize);
        let mut disk: Vec<u8> = (0..sectors * SECTOR).map(|_| rng.next() as u8).collect();
        // Half with a valid signature and revision in both header slots.
        if sectors >= 3 && rng.below(2) == 0 {
            for at in [SECTOR, (sectors - 1) * SECTOR] {
                disk[at..at + 8].copy_from_slice(b"EFI PART");
                disk[at + 8..at + 12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
                disk[at + 12..at + 16].copy_from_slice(&92u32.to_le_bytes());
            }
        }
        harness::gpt_read(&disk);
    }
}

/// Where each header field starts: revision, size, CRC, my and the other LBA,
/// the usable range, the disk GUID, the array's LBA, count, entry size and CRC.
const HEADER_FIELDS: [usize; 12] = [8, 12, 16, 24, 32, 40, 48, 56, 72, 80, 84, 88];
/// Where each entry field starts: type, GUID, first, last, attributes, name.
const ENTRY_FIELDS: [usize; 6] = [0, 16, 32, 40, 48, 56];

#[test]
fn every_header_and_entry_field_at_every_extreme() {
    let disk = sample();
    let mut starts = Vec::new();
    for header in [PRIMARY, BACKUP] {
        starts.extend(HEADER_FIELDS.iter().map(|f| header + f));
    }
    for entry in 1..3 {
        // A and B: the slots rollback chooses between.
        starts.extend(ENTRY_FIELDS.iter().map(|f| ENTRIES.start + entry * 128 + f));
    }
    for at in starts {
        sweep_fields(&disk, at..at + 1, |_, mutated| harness::gpt_read(mutated));
    }
}

#[test]
fn random_mutations_of_the_headers_and_entries() {
    let disk = sample();
    // Mutate the first sectors (MBR, primary header, start of the array) and
    // the backup header; keep the length, so the backup stays where it is.
    for (i, range) in [0..ENTRIES.end, BACKUP..BACKUP + SECTOR]
        .into_iter()
        .enumerate()
    {
        fuzz(&disk[range.clone()], 0x6a00 + i as u64, 400, |part| {
            let mut whole = disk.clone();
            let n = part.len().min(range.len());
            whole[range.start..range.start + n].copy_from_slice(&part[..n]);
            harness::gpt_read(&whole);
        });
    }
}

#[test]
fn rewritten_crcs_still_refuse_a_misleading_table() {
    let base = sample();
    let entry = |i: usize| 2 * SECTOR + i * 128;
    let put = |d: &mut Vec<u8>, at: usize, v: u64| d[at..at + 8].copy_from_slice(&v.to_le_bytes());
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    // B over A, a slot past the usable end, a slot ending before it starts,
    // an array that runs off the disk and one over the usable range, and a
    // usable range over the backup header.
    let mut d = base.clone();
    put(&mut d, entry(2) + 32, FIRST_USABLE + 15);
    cases.push(("overlap", d));
    let mut d = base.clone();
    put(&mut d, entry(1) + 40, SECTORS);
    cases.push(("past the end", d));
    let mut d = base.clone();
    put(&mut d, entry(1) + 32, FIRST_USABLE + 30);
    cases.push(("backwards", d));
    let mut d = base.clone();
    put(&mut d, PRIMARY + 72, SECTORS - 2);
    cases.push(("array off the disk", d));
    let mut d = base.clone();
    put(&mut d, PRIMARY + 72, FIRST_USABLE + 1);
    cases.push(("array over the usable range", d));
    let mut d = base.clone();
    put(&mut d, PRIMARY + 48, SECTORS - 1);
    cases.push(("usable over the backup", d));
    for (what, mut disk) in cases {
        // Only the primary is changed; rewrite its CRCs and spoil the backup,
        // so nothing can be read from the backup instead.
        assert!(harness::recrc(&mut disk), "{what}");
        disk[BACKUP] ^= 0xFF;
        let mut image = harness::Image::new(&disk);
        assert!(read(&mut image).is_err(), "{what}");
        assert_eq!(image.refused, 0, "{what}");
        harness::gpt_read(&disk);
    }
    // The sample itself still reads after a rewrite: the rewrite changes
    // nothing that was right.
    let mut disk = base.clone();
    harness::recrc(&mut disk);
    assert_eq!(disk, base);
}

/// The backup array's first four entries, where the backup header says it is.
fn backup_entries(disk: &[u8]) -> std::ops::Range<usize> {
    let lba = u64::from_le_bytes(disk[BACKUP + 72..BACKUP + 80].try_into().unwrap()) as usize;
    lba * SECTOR..lba * SECTOR + 4 * 128
}

/// With the primary header spoiled (its signature, which no CRC rewrite
/// mends), only the backup's array can be read, so every mutation of it
/// reaches the entry checks or is refused.
fn backup_only(disk: &mut [u8]) {
    disk[PRIMARY] ^= 0xFF;
}

#[test]
fn the_backup_array_is_the_sample_s_and_a_damaged_one_is_refused() {
    let mut disk = sample();
    let entries = backup_entries(&disk);
    assert_ne!(entries.start, 2 * SECTOR, "the backup has its own array");
    assert_eq!(disk[entries.clone()], disk[ENTRIES], "the same entries");
    backup_only(&mut disk);
    let table = harness::check(&disk).expect("the backup reads");
    assert!(table.from_backup && table.partitions.len() == 4);
    // One flipped bit in the backup array, its CRC left: refused, and the
    // primary is spoiled too, so nothing reads.
    disk[entries.start + 40] ^= 1;
    assert!(harness::check(&disk).is_none());
    // Rewritten, the entry now ends a sector early and is the table's.
    assert!(harness::recrc(&mut disk));
    let table = harness::check(&disk).expect("a rewritten backup reads");
    assert!(table.from_backup);
}

#[test]
fn mutated_backup_arrays_never_mislead() {
    let disk = sample();
    let entries = backup_entries(&disk);
    // Every field of the A and B slots' entries.
    for entry in 1..3 {
        for f in ENTRY_FIELDS {
            let at = entries.start + entry * 128 + f;
            sweep_fields(&disk, at..at + 1, |_, mutated| {
                let mut spoiled = mutated.to_vec();
                backup_only(&mut spoiled);
                harness::gpt_read(&spoiled);
            });
        }
    }
    // Random bytes over the first four entries, then over the backup header.
    for (i, range) in [entries.clone(), BACKUP..BACKUP + SECTOR]
        .into_iter()
        .enumerate()
    {
        fuzz(&disk[range.clone()], 0x6b00 + i as u64, 400, |part| {
            let mut whole = disk.clone();
            let n = part.len().min(range.len());
            whole[range.start..range.start + n].copy_from_slice(&part[..n]);
            backup_only(&mut whole);
            harness::gpt_read(&whole);
        });
    }
}
