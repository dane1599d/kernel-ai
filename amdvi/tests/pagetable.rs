//! I/O page tables over a fake DMA allocator: maps and unmaps are all or
//! none, tables are freed when empty, and a table this crate did not write
//! is never trusted.

mod kit;

#[path = "../../testkit/hostile.rs"]
mod hostile;

use amdvi::pagetable::{Access, DmaAlloc, ENTRIES, IoPageTable, MapError, PAGE};
use kit::FakeDma;

const PR: u64 = 1;
const IR_IW: u64 = 1 << 61 | 1 << 62;

fn table(dma: &mut FakeDma) -> IoPageTable {
    IoPageTable::new(dma, 4).unwrap()
}

#[test]
fn one_page_at_four_levels_has_a_table_per_level() {
    let mut dma = FakeDma::new();
    let mut t = table(&mut dma);
    // The indexes of bits 47:39, 38:30, 29:21 and 20:12.
    let indexes = [0x101, 0x000, 0x100, 0x103];
    let iova = 0x101 << 39 | 0x100 << 21 | 0x103 << 12;
    t.map(&mut dma, iova, 0x7654_3000, 1, Access::READ_WRITE)
        .unwrap();
    assert_eq!(dma.live(), 4);
    let mut at = t.root();
    for (level, index) in (1..=4u64).rev().zip(indexes) {
        let entry = dma.read(at, index);
        if level == 1 {
            assert_eq!(entry, PR | 0x7654_3000 | IR_IW);
        } else {
            assert_eq!(
                entry & !0x000f_ffff_ffff_f000,
                PR | (level - 1) << 9 | IR_IW
            );
            at = entry & 0x000f_ffff_ffff_f000;
        }
    }
    assert_eq!(
        t.translate(&dma, iova + 0x123),
        Some((0x7654_3123, Access::READ_WRITE))
    );
    assert_eq!(t.translate(&dma, iova + PAGE), None);
}

#[test]
fn a_range_across_tables_maps_and_unmaps_back_to_the_root() {
    let mut dma = FakeDma::new();
    let mut t = table(&mut dma);
    // Two pages either side of a level-1 table's end (root, one table at
    // levels 3 and 2, two at level 1), then either side of a level-2's.
    t.map(&mut dma, 0x1f_f000, 0x20_0000, 2, Access::READ)
        .unwrap();
    assert_eq!(dma.live(), 5);
    t.map(&mut dma, 0x3fff_f000, 0x40_0000, 2, Access::READ_WRITE)
        .unwrap();
    assert_eq!(dma.live(), 8);
    assert_eq!(
        t.translate(&dma, 0x20_0000),
        Some((0x20_1000, Access::READ))
    );
    assert_eq!(
        t.translate(&dma, 0x4000_0000),
        Some((0x40_1000, Access::READ_WRITE))
    );
    t.unmap(&mut dma, 0x1f_f000, 2).unwrap();
    // Its two level-1 tables go; the level-2 one holds the other range.
    assert_eq!(dma.live(), 6);
    t.unmap(&mut dma, 0x3fff_f000, 2).unwrap();
    assert_eq!(dma.live(), 1, "only the root is left");
    assert_eq!(t.translate(&dma, 0x4000_0000), None);
    t.destroy(&mut dma);
    assert_eq!(dma.live(), 0);
}

#[test]
fn a_map_over_a_mapped_page_or_an_unmap_of_a_missing_one_changes_nothing() {
    let mut dma = FakeDma::new();
    let mut t = table(&mut dma);
    t.map(&mut dma, 0x5000, 0x9000, 1, Access::READ).unwrap();
    let before = dma.frames.clone();
    assert_eq!(
        t.map(&mut dma, 0x3000, 0x1_0000, 4, Access::READ),
        Err(MapError::Mapped(0x5000))
    );
    assert_eq!(
        t.unmap(&mut dma, 0x4000, 3),
        Err(MapError::NotMapped(0x4000))
    );
    assert_eq!(dma.frames, before);
}

#[test]
fn running_out_of_tables_part_way_maps_nothing_and_keeps_nothing() {
    // The root, then three tables for the first page's path, and none for
    // the second level-1 table the range reaches.
    let mut dma = FakeDma::with_frames(4);
    let mut t = table(&mut dma);
    assert_eq!(
        t.map(&mut dma, 0x1f_e000, 0x5000, 4, Access::READ),
        Err(MapError::NoMemory)
    );
    assert_eq!(dma.live(), 1);
    assert!((0..ENTRIES).all(|i| dma.read(t.root(), i) == 0));
    // Short of a deeper table: what the shortage left made is freed too.
    let mut dma = FakeDma::with_frames(3);
    let mut t = table(&mut dma);
    assert_eq!(
        t.map(&mut dma, 0, 0x5000, 1, Access::READ),
        Err(MapError::NoMemory)
    );
    assert_eq!(dma.live(), 1);
}

#[test]
fn bad_ranges_are_refused() {
    let mut dma = FakeDma::new();
    let mut t = table(&mut dma);
    let rw = Access::READ_WRITE;
    assert_eq!(t.limit(), Some(1 << 48));
    assert_eq!(t.map(&mut dma, 0x1001, 0, 1, rw), Err(MapError::Unaligned));
    assert_eq!(
        t.map(&mut dma, 0x1000, 0x10, 1, rw),
        Err(MapError::Unaligned)
    );
    assert_eq!(t.map(&mut dma, 0x1000, 0, 0, rw), Err(MapError::Unaligned));
    assert_eq!(
        t.map(&mut dma, 1 << 48, 0, 1, rw),
        Err(MapError::OutOfRange)
    );
    assert_eq!(
        t.map(&mut dma, (1 << 48) - PAGE, 0, 2, rw),
        Err(MapError::OutOfRange)
    );
    assert_eq!(
        t.map(&mut dma, 0, (1 << 52) - PAGE, 2, rw),
        Err(MapError::OutOfRange)
    );
    assert_eq!(t.map(&mut dma, !0xfff, 0, 2, rw), Err(MapError::OutOfRange));
    assert_eq!(t.map(&mut dma, 0, !0xfff, 2, rw), Err(MapError::OutOfRange));
    assert_eq!(
        t.map(&mut dma, 0, 1 << 52, 1, rw),
        Err(MapError::OutOfRange)
    );
    assert_eq!(
        t.map(
            &mut dma,
            0,
            0,
            1,
            Access {
                read: false,
                write: false
            }
        ),
        Err(MapError::NoAccess)
    );
    assert_eq!(dma.live(), 1);
    for levels in [0, 7] {
        assert_eq!(
            IoPageTable::new(&mut dma, levels),
            Err(MapError::Levels(levels))
        );
    }
    let three = IoPageTable::new(&mut dma, 3).unwrap();
    assert_eq!(three.limit(), Some(1 << 39));
}

#[test]
fn a_table_with_an_entry_it_did_not_write_is_not_followed() {
    let mut dma = FakeDma::new();
    let mut t = table(&mut dma);
    t.map(&mut dma, 0, 0x5000, 1, Access::READ).unwrap();
    // The root's entry says its table is level 1, not 3, then points at itself.
    let root = t.root();
    let entry = dma.read(root, 0);
    for bad in [entry & !(7 << 9) | 1 << 9, PR | 3 << 9 | root | IR_IW] {
        dma.write(root, 0, bad);
        assert_eq!(t.translate(&dma, 0), None, "{bad:#x}");
        assert_eq!(
            t.map(&mut dma, 0x1000, 0x6000, 1, Access::READ),
            Err(MapError::Corrupt(0x1000)),
            "{bad:#x}"
        );
        assert_eq!(t.unmap(&mut dma, 0, 1), Err(MapError::Corrupt(0)));
    }
}

/// Tables of random words: every walk ends, and nothing panics.
struct Garbage(hostile::XorShift, u64);

impl DmaAlloc for Garbage {
    fn alloc(&mut self) -> Option<u64> {
        (self.0.below(4) != 0).then(|| (self.0.next() & 0xfff_ffff_f000) | 0x1000)
    }
    fn free(&mut self, _: u64) {}
    fn read(&self, phys: u64, index: usize) -> u64 {
        // The same word each time for the same place.
        let mut rng = hostile::XorShift::new(self.1 ^ phys ^ (index as u64) << 52);
        let word = rng.next();
        match rng.below(3) {
            0 => word,
            1 => 0,
            _ => word & !(7 << 9) | (rng.below(4) as u64) << 9 | 1,
        }
    }
    fn write(&mut self, _: u64, _: usize, _: u64) {}
}

#[test]
fn walks_of_hostile_tables_end() {
    for seed in 1..200u64 {
        let mut a = Garbage(hostile::XorShift::new(seed), seed);
        let Ok(mut t) = IoPageTable::new(&mut a, 4) else {
            continue;
        };
        for n in 0..50u64 {
            let iova = (a.0.next() & 0xffff_ffff_f000) | n << 12 & 0x3f000;
            let _ = t.translate(&a, iova);
            let _ = t.map(&mut a, iova, 0x1000, 3, Access::READ);
            let _ = t.unmap(&mut a, iova, 2);
        }
    }
}
