//! Guest RAM's layout over one `Shm` and the nested table it builds (APM
//! vol. 2 rev. 3.38, 15.25): each region checked as added, and every page
//! of it translating to its own `Shm` frame.

mod common;

use common::{Contiguous, GIB, MIB, Pool, Scattered, guest_ram};
use svm::npt::{
    Access, GUEST_LIMIT, Layout, NptEntry, Refusal, Region, Size, Tables, install, tables_for,
    translate,
};

fn region(gpa: u64, bytes: u64, offset: u64) -> Region {
    Region {
        gpa,
        bytes,
        offset,
        access: Access::RAM,
    }
}

#[test]
fn a_region_is_refused_when_misaligned_empty_or_too_far() {
    let mut layout = Layout::new(4 * GIB, 64 * MIB).unwrap();
    for bad in [
        region(0x800, 4096, 0),
        region(0, 4095, 0),
        region(0, 4096, 0x10),
    ] {
        assert_eq!(layout.add(bad), Err(Refusal::Misaligned(bad)));
    }
    let empty = region(0, 0, 0);
    assert_eq!(layout.add(empty), Err(Refusal::Empty(empty)));
    let past_guest = region(4 * GIB - 4096, 8192, 0);
    assert_eq!(layout.add(past_guest), Err(Refusal::PastGuest(past_guest)));
    let wraps = region(0xFFFF_FFFF_FFFF_F000, 4096, 0);
    assert_eq!(layout.add(wraps), Err(Refusal::PastGuest(wraps)));
    let past_shm = region(0, 64 * MIB, 4096);
    assert_eq!(layout.add(past_shm), Err(Refusal::PastShm(past_shm)));
    // Nothing refused was kept.
    assert_eq!(layout.regions().count(), 0);
    assert_eq!(
        Layout::new(4 * GIB + 1, 0),
        Err(Refusal::GuestSize(4 * GIB + 1))
    );
    assert_eq!(
        Layout::new(GUEST_LIMIT + 4096, 0),
        Err(Refusal::GuestSize(GUEST_LIMIT + 4096))
    );
    assert_eq!(Layout::new(GIB, 100), Err(Refusal::ShmSize(100)));
}

#[test]
fn regions_may_not_overlap_in_the_guest_or_in_the_shm() {
    let mut layout = Layout::new(8 * GIB, 2 * GIB).unwrap();
    let low = region(0, GIB, 0);
    layout.add(low).unwrap();
    // The same guest pages from elsewhere in the Shm.
    let again = region(GIB - 4096, 8192, GIB);
    assert_eq!(
        layout.add(again),
        Err(Refusal::Overlap {
            region: again,
            index: 0
        })
    );
    // Other guest pages aliasing the same Shm pages.
    let alias = region(4 * GIB, 4096, GIB - 4096);
    assert_eq!(
        layout.add(alias),
        Err(Refusal::Overlap {
            region: alias,
            index: 0
        })
    );
    // Touching is not overlapping.
    let high = region(4 * GIB, GIB, GIB);
    layout.add(high).unwrap();
    assert_eq!(layout.find(GIB - 1), Some(low));
    assert_eq!(layout.find(GIB), None);
    assert_eq!(layout.find(4 * GIB), Some(high));
    assert_eq!(layout.find(5 * GIB), None);
}

#[test]
fn a_layout_holds_a_fixed_number_of_regions() {
    let mut layout = Layout::new(GIB, GIB).unwrap();
    for n in 0..svm::npt::REGIONS as u64 {
        layout.add(region(n * 2 * MIB, MIB, n * MIB)).unwrap();
    }
    assert_eq!(
        layout.add(region(GIB - MIB, MIB, GIB - MIB)),
        Err(Refusal::Full)
    );
}

#[test]
fn guest_ram_maps_in_2_mib_leaves_with_the_tables_counted_at_create() {
    let ram = guest_ram();
    let shm = Contiguous {
        base: 0x2_0000_0000,
        bytes: 1536 * MIB,
    };
    assert!(ram.tables() >= tables_for(1536 * MIB));
    let mut pool = Pool::new(tables_for(1536 * MIB) as usize);
    let root = pool.alloc().unwrap();
    ram.map(&shm, &mut pool, root).unwrap();
    // Exactly what `tables_for` sized: PML4, PDPT and two PDs.
    assert_eq!(pool.tables.len(), 4);
    for gpa in [0, 0x1234, 2 * MIB, GIB + 0x10_0FFF, 1536 * MIB - 1] {
        assert_eq!(
            translate(&pool, root, gpa),
            Some((shm.base + gpa, Access::RAM)),
            "{gpa:#x}"
        );
    }
    for gpa in [1536 * MIB, 3 * GIB, GUEST_LIMIT, u64::MAX] {
        assert_eq!(translate(&pool, root, gpa), None, "{gpa:#x}");
    }
}

#[test]
fn scattered_frames_map_page_by_page() {
    let mut ram = Layout::new(4 * GIB, 4 * MIB).unwrap();
    ram.add(region(0x10_0000, 4 * MIB, 0)).unwrap();
    let shm = Scattered {
        base: 0x3_0000_0000,
        stride: 3 * 4096,
        bytes: 4 * MIB,
    };
    // Sized by the worst case: scattered frames take a PT a 2 MiB.
    let mut pool = Pool::new(ram.tables() as usize);
    let root = pool.alloc().unwrap();
    ram.map(&shm, &mut pool, root).unwrap();
    // The worst case, reached: a root, a PDPT, a PD and 3 PTs.
    assert_eq!(ram.tables(), 6);
    assert_eq!(pool.tables.len(), 6);
    for page in 0..1024 {
        let gpa = 0x10_0000 + page * 4096 + 8;
        assert_eq!(
            translate(&pool, root, gpa),
            Some((shm.base + page * shm.stride + 8, Access::RAM))
        );
    }
    assert_eq!(translate(&pool, root, 0x10_0000 - 1), None);
    assert_eq!(translate(&pool, root, 0x50_0000), None);
}

#[test]
fn ram_and_a_rom_around_the_pci_hole_keep_their_rights() {
    let mut ram = Layout::new(8 * GIB, 3 * GIB + 2 * MIB).unwrap();
    ram.add(region(0, 3 * GIB, 0)).unwrap();
    ram.add(Region {
        gpa: 4 * GIB - 2 * MIB,
        bytes: 2 * MIB,
        offset: 3 * GIB,
        access: Access::READ,
    })
    .unwrap();
    let shm = Contiguous {
        base: 0x4_0000_0000,
        bytes: 3 * GIB + 2 * MIB,
    };
    let mut pool = Pool::new(16);
    let root = pool.alloc().unwrap();
    ram.map(&shm, &mut pool, root).unwrap();
    assert_eq!(
        translate(&pool, root, 3 * GIB - 1),
        Some((shm.base + 3 * GIB - 1, Access::RAM))
    );
    assert_eq!(translate(&pool, root, 3 * GIB), None);
    assert_eq!(
        translate(&pool, root, 4 * GIB - 4096),
        Some((shm.base + 3 * GIB + 2 * MIB - 4096, Access::READ))
    );
}

#[test]
fn mapping_refuses_a_spent_pool_missing_frames_and_a_mapped_address() {
    let ram = guest_ram();
    let shm = Contiguous {
        base: 0x2_0000_0000,
        bytes: 1536 * MIB,
    };
    let mut small = Pool::new(3);
    let root = small.alloc().unwrap();
    assert_eq!(ram.map(&shm, &mut small, root), Err(Refusal::NoTables));
    // A Shm shorter than the layout says: no frame for the last page.
    let short = Contiguous {
        base: 0x2_0000_0000,
        bytes: MIB,
    };
    let mut pool = Pool::new(8);
    let root = pool.alloc().unwrap();
    assert_eq!(ram.map(&short, &mut pool, root), Err(Refusal::Backing(MIB)));
    // A leaf over a leaf, and a 4 KiB leaf under a 2 MiB one.
    let mut pool = Pool::new(8);
    let root = pool.alloc().unwrap();
    let leaf = NptEntry::leaf(0x20_0000, Size::Page2M, Access::RAM).unwrap();
    install(&mut pool, root, 0, leaf, Size::Page2M).unwrap();
    assert_eq!(
        install(&mut pool, root, 0, leaf, Size::Page2M),
        Err(Refusal::Mapped(0))
    );
    let small = NptEntry::leaf(0x1000, Size::Page4K, Access::RAM).unwrap();
    assert_eq!(
        install(&mut pool, root, 0x1000, small, Size::Page4K),
        Err(Refusal::Mapped(0x1000))
    );
}

#[test]
fn ram_may_not_cover_an_apic() {
    let mut layout = Layout::new(4 * GIB, 64 * MIB).unwrap();
    for gpa in [0xFEC0_0000, 0xFEDF_F000, 0xFEE0_0000] {
        let over = region(gpa, 2 * MIB, 0);
        assert_eq!(layout.add(over), Err(Refusal::Apic(over)), "{gpa:#x}");
    }
    let under = region(0xFEC0_0000, 4096, 0);
    assert_eq!(layout.add(under), Err(Refusal::Apic(under)));
    // Up to the IOAPIC, and from past the LAPIC, is fine.
    layout.add(region(0xFEA0_0000, 2 * MIB, 0)).unwrap();
    layout.add(region(0xFEE0_1000, 4096, 2 * MIB)).unwrap();
}

#[test]
fn the_worst_case_counts_every_level_a_region_touches() {
    // One page: a root, a PDPT, a PD and a PT.
    let mut one = Layout::new(4 * GIB, 4096).unwrap();
    one.add(region(0x1000, 4096, 0)).unwrap();
    assert_eq!(one.tables(), 4);
    // 4 MiB across a 1 GiB line: 3 PTs, 2 PDs, 1 PDPT, and the root.
    let mut across = Layout::new(4 * GIB, 4 * MIB).unwrap();
    across
        .add(region(GIB - 2 * MIB + 4096, 4 * MIB, 0))
        .unwrap();
    assert_eq!(across.tables(), 1 + 3 + 2 + 1);
    assert_eq!(Layout::new(GIB, 0).unwrap().tables(), 1);
}
