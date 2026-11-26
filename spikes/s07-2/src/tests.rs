//! The formats against the bits QEMU's `amd_iommu.c` checks.

extern crate std;

use super::*;
use std::collections::BTreeMap;
use std::vec::Vec;

#[test]
fn a_dte_is_valid_translated_four_level_and_permits_read_and_write() {
    let [first, second, third, fourth] = dte(0x0012_3000, 4, 1);
    assert_eq!(first & 0b11, 0b11);
    assert_eq!(dte_levels(first), 4);
    assert_eq!(first & ADDRESS, 0x0012_3000);
    assert!(first & IR != 0 && first & IW != 0);
    // QEMU's AMDVI_DTE_QUAD0_RESERVED: bits 2-7, 52-53 and 63 stay clear.
    assert_eq!(first & (0xFC | 0b11 << 52 | 1 << 63), 0);
    assert_eq!((second, third, fourth), (1, 0, 0));
}

#[test]
fn device_ids_are_bus_device_function() {
    assert_eq!(device_id(0, 2, 0), 0x0010);
    assert_eq!(device_id(0, 0x1F, 7), 0x00FF);
    assert_eq!(device_id(1, 0, 0), 0x0100);
}

#[test]
fn commands_carry_their_opcode_and_no_reserved_bits() {
    let wait = completion_wait(0x0010_0008, 0xC0FFEE);
    assert_eq!(wait[0] >> 60, opcode::COMPLETION_WAIT);
    assert_eq!(wait[0] & 1, 1);
    assert_eq!(wait[0] & ADDRESS, 0x0010_0000);
    assert_eq!(wait[1], 0xC0FFEE);
    // QEMU refuses a COMPLETION_WAIT with bits 52-59 set.
    assert_eq!(wait[0] >> 52 & 0xFF, 0);
    let dte = invalidate_dte(0x0018);
    assert_eq!(dte, [0x0018 | 2 << 60, 0]);
    let pages = invalidate_domain(1);
    assert_eq!(pages[0] >> 60, opcode::INVALIDATE_IOMMU_PAGES);
    assert_eq!(pages[0] >> 32 & 0xFFFF, 1);
    // QEMU's reserved checks: cmd[0] bits 20-31 and 48-59, cmd[1] bits 3-11.
    assert_eq!(pages[0] >> 20 & 0xFFF, 0);
    assert_eq!(pages[0] >> 48 & 0xFFF, 0);
    assert_eq!(pages[1] >> 3 & 0x1FF, 0);
}

#[test]
fn ring_and_table_registers_hold_their_sizes() {
    assert_eq!(devtab_base(0x0020_0000), 0x0020_0000 | 0x1FF);
    assert_eq!(ring_base(0x0030_0000), 0x0030_0000 | 8 << 56);
    assert_eq!(RING_BYTES / ENTRY_BYTES, 1 << RING_LOG2);
}

/// A fake memory of tables for [`identity`] and [`translate`].
#[derive(Default)]
struct Tables {
    next: u64,
    entries: BTreeMap<(u64, usize), u64>,
    tables: Vec<u64>,
}

impl Tables {
    fn build(&mut self, top: u64, hole: u64, limit: usize) -> Option<u64> {
        let cell = core::cell::RefCell::new(self);
        let mut alloc = || {
            let mut t = cell.borrow_mut();
            if t.tables.len() == limit {
                return None;
            }
            t.next += 4096;
            let at = 0x7000_0000_0000 + t.next;
            t.tables.push(at);
            Some(at)
        };
        let mut store = |table: u64, index: usize, entry: u64| {
            assert!(index < 512);
            cell.borrow_mut().entries.insert((table, index), entry);
        };
        identity(top, hole, &mut alloc, &mut store)
    }

    fn load(&self) -> impl Fn(u64, usize) -> u64 + '_ {
        |table, index| self.entries.get(&(table, index)).copied().unwrap_or(0)
    }
}

#[test]
fn identity_maps_everything_but_the_hole() {
    const MIB: u64 = 1 << 20;
    let mut tables = Tables::default();
    let hole = 1026 * MIB;
    let root = tables.build(3 * 1024 * MIB + 1, hole, 16).unwrap();
    // A root, one level-3 table and four level-2 tables (the top rounds up).
    assert_eq!(tables.tables.len(), 1 + 1 + 4);
    let first = dte(root, 4, 1)[0];
    let load = tables.load();
    for iova in [
        0,
        0x1234,
        2 * MIB - 1,
        hole - 1,
        hole + 2 * MIB,
        3072 * MIB,
        3074 * MIB - 1,
    ] {
        assert_eq!(
            translate(first, iova, &load),
            Some((iova, true)),
            "{iova:#x}"
        );
    }
    for iova in [
        hole,
        hole + 0x1000,
        hole + 2 * MIB - 1,
        3074 * MIB,
        1 << 39,
        u64::MAX,
    ] {
        assert_eq!(translate(first, iova, &load), None, "{iova:#x}");
    }
    // A table that is not valid translates nothing.
    assert_eq!(translate(first & !1, 0, &load), None);
}

#[test]
fn identity_refuses_an_unaligned_hole_and_a_spent_pool() {
    let mut tables = Tables::default();
    assert_eq!(tables.build(1 << 30, 0x1000, 16), None);
    let mut small = Tables::default();
    assert_eq!(small.build(4 << 30, 0, 3), None);
}

#[test]
fn a_page_fault_event_decodes() {
    // As QEMU's amdvi_encode_event writes one: devid, then info << 48, where
    // IO_PAGE_FAULT's info is 0x2 << 12 with its flags in the low bits.
    let info: u64 = 0x2 << 12 | 0x20;
    let event = Event::decode([0x0018 | info << 48, 0x0040_0000]);
    assert_eq!(event.code, IO_PAGE_FAULT);
    assert_eq!(event.device, 0x0018);
    assert_eq!(event.flags, 0x20);
    assert_eq!(event.address, 0x0040_0000);
    assert_eq!(event.name(), "IO_PAGE_FAULT");
}
