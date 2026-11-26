//! Nested entries in the long-mode format (APM vol. 2, 5.3 and 15.25): P 0,
//! R/W 1, U/S 2, PS 7, NX 63, the address in 51:12.

use crate::npt::{
    ADDRESS, Access, MAX_PHYSICAL, NX, NptEntry, P, PS, RW, Size, Table, US, indices, ncr3,
    tables_for,
};
use core::mem::{align_of, size_of};

#[test]
fn a_table_entry_is_present_writable_user_and_executable() {
    let entry = NptEntry::table(0x0012_3000).unwrap();
    assert_eq!(entry.0, 0x0012_3000 | P | RW | US);
    assert!(entry.present() && entry.writable() && entry.executable() && !entry.huge());
    assert_eq!(entry.address(), 0x0012_3000);
    assert_eq!(NptEntry::table(0x0012_3008), None);
    assert_eq!(NptEntry::table(MAX_PHYSICAL + 1), None);
}

#[test]
fn a_leaf_carries_its_rights_and_size() {
    let ram = NptEntry::leaf(0x0040_0000, Size::Page2M, Access::RAM).unwrap();
    assert_eq!(ram.0, 0x0040_0000 | P | RW | US | PS);
    assert!(ram.huge() && ram.writable() && ram.executable());
    let rom = NptEntry::leaf(0x000F_0000, Size::Page4K, Access::READ).unwrap();
    assert_eq!(rom.0, 0x000F_0000 | P | US | NX);
    assert!(!rom.writable() && !rom.executable() && !rom.huge());
    let gig = NptEntry::leaf(0x4000_0000, Size::Page1G, Access::RAM).unwrap();
    assert_eq!(gig.address(), 0x4000_0000);
    // Each size wants its own alignment.
    assert_eq!(NptEntry::leaf(0x1000, Size::Page2M, Access::RAM), None);
    assert_eq!(NptEntry::leaf(0x0020_0000, Size::Page1G, Access::RAM), None);
    assert!(NptEntry::leaf(0x1000, Size::Page4K, Access::RAM).is_some());
    // Every entry is a user entry: nested walks are user accesses.
    for entry in [ram, rom, gig, NptEntry::table(0x6000).unwrap()] {
        assert!(entry.0 & US != 0);
        assert_eq!(entry.0 & ADDRESS & !(1 << 12), entry.address());
    }
    assert!(!NptEntry::EMPTY.present());
}

#[test]
fn a_walk_takes_nine_bits_a_level_and_ncr3_wants_a_page() {
    assert_eq!(indices(0), [0, 0, 0, 0]);
    assert_eq!(indices(0xFEE0_0000), [0, 3, 0x1F7, 0]);
    assert_eq!(indices(0x0080_0040_1000), [1, 0, 2, 1]);
    assert_eq!(indices(0x0000_FFFF_FFFF_F000), [511, 511, 511, 511]);
    assert_eq!(ncr3(0x0010_0000), Some(0x0010_0000));
    assert_eq!(ncr3(0x0010_0800), None);
    assert_eq!(size_of::<Table>(), 4096);
    assert_eq!(align_of::<Table>(), 4096);
}

#[test]
fn guest_ram_in_2_mib_leaves_needs_a_pd_a_gib() {
    // 1.5 GiB of guest RAM (ADR 0006 item 9): PML4, PDPT and two PDs.
    assert_eq!(tables_for(1536 << 20), 4);
    assert_eq!(tables_for(1 << 30), 3);
    assert_eq!(tables_for(513 << 30), 1 + 2 + 513);
}
