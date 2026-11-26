//! Intercept bits at table B-1's word and bit, and the permission maps'
//! layout from APM vol. 2, 15.10.1 and 15.11.

use crate::intercept::{
    IOPM_SIZE, Intercept, Intercepts, IoPermissions, MSRPM_SIZE, MsrPermissions, msr_bit,
};
use crate::vmcb::Vmcb;
use core::mem::size_of;

/// (intercept, word, bit) as table B-1 gives them.
#[test]
fn intercepts_are_at_table_b1s_word_and_bit() {
    let table: &[(Intercept, usize, u32)] = &[
        (Intercept::cr_read(0), 0, 0),
        (Intercept::cr_read(8), 0, 8),
        (Intercept::cr_write(3), 0, 19),
        (Intercept::dr_read(7), 1, 7),
        (Intercept::dr_write(7), 1, 23),
        (Intercept::exception(14), 2, 14),
        (Intercept::INTR, 3, 0),
        (Intercept::NMI, 3, 1),
        (Intercept::CPUID, 3, 18),
        (Intercept::HLT, 3, 24),
        (Intercept::IOIO_PROT, 3, 27),
        (Intercept::MSR_PROT, 3, 28),
        (Intercept::SHUTDOWN, 3, 31),
        (Intercept::VMRUN, 4, 0),
        (Intercept::VMMCALL, 4, 1),
        (Intercept::XSETBV, 4, 13),
        (Intercept::EFER_WRITE_TRAP, 4, 15),
        (Intercept::cr_write_trap(0), 4, 16),
        (Intercept::INVPCID, 5, 2),
        (Intercept::BUS_LOCK, 5, 5),
    ];
    for &(intercept, word, bit) in table {
        let set = Intercepts::none().with(intercept);
        let mut want = [0u32; 6];
        want[word] = 1 << bit;
        assert_eq!(set.0, want, "{intercept:?}");
    }
}

#[test]
fn a_set_adds_removes_and_lands_in_the_vmcb() {
    let set = Intercepts::required()
        .with(Intercept::HLT)
        .with(Intercept::CPUID)
        .with(Intercept::MSR_PROT);
    assert!(set.contains(Intercept::VMRUN) && set.contains(Intercept::SHUTDOWN));
    assert!(set.contains(Intercept::HLT));
    let set = set.without(Intercept::HLT);
    assert!(!set.contains(Intercept::HLT) && set.contains(Intercept::CPUID));
    let mut vmcb = Vmcb::zeroed();
    set.apply(&mut vmcb.control);
    assert_eq!(Intercepts::of(&vmcb.control), set);
    // Word 4, bit 0 is VMRUN: what VMRUN checks first.
    assert_eq!(vmcb.control.intercepts[4] & 1, 1);
}

#[test]
fn an_msrs_two_bits_are_in_its_range_of_the_map() {
    assert_eq!(size_of::<MsrPermissions>(), MSRPM_SIZE);
    // Range 0: MSR 0 at byte 0; MSR 0x1FFF at the range's last byte.
    assert_eq!(msr_bit(0x0000_0000), Some((0x000, 0)));
    assert_eq!(msr_bit(0x0000_0003), Some((0x000, 6)));
    assert_eq!(msr_bit(0x0000_1FFF), Some((0x7FF, 6)));
    // IA32_APIC_BASE (0x1B): bit 54 of range 0.
    assert_eq!(msr_bit(0x1B), Some((6, 6)));
    // EFER (0xC000_0080) in range 1 at 0x800.
    assert_eq!(msr_bit(0xC000_0080), Some((0x800 + 0x20, 0)));
    // VM_HSAVE_PA (0xC001_0117) in range 2 at 0x1000.
    assert_eq!(msr_bit(0xC001_0117), Some((0x1000 + 0x45, 6)));
    // Outside the three ranges: no bit, always intercepted.
    assert_eq!(msr_bit(0x0000_2000), None);
    assert_eq!(msr_bit(0xC000_2000), None);
    assert_eq!(msr_bit(0x4000_0000), None);
}

#[test]
fn the_msr_map_opens_only_what_it_is_told() {
    let mut map = MsrPermissions::intercept_all();
    assert_eq!(map.intercepts(0xC000_0100), (true, true));
    // FS.base: the guest reads and writes it itself.
    map.allow(0xC000_0100, true, true).unwrap();
    assert_eq!(map.intercepts(0xC000_0100), (false, false));
    // TSC: reads pass, writes exit.
    map.allow(0x10, true, false).unwrap();
    assert_eq!(map.intercepts(0x10), (false, true));
    // Its neighbours are untouched.
    assert_eq!(map.intercepts(0x11), (true, true));
    assert_eq!(map.intercepts(0xC000_0101), (true, true));
    assert_eq!(map.allow(0x4000_0000, true, true), Err(0x4000_0000));
    assert_eq!(map.intercepts(0x4000_0000), (true, true));
}

#[test]
fn the_io_map_has_a_bit_a_port_and_checks_every_byte() {
    assert_eq!(size_of::<IoPermissions>(), IOPM_SIZE);
    let mut map = IoPermissions::intercept_all();
    assert!(map.intercepts(0x3F8, 1));
    map.allow(0x3F8, 8);
    assert!(!map.intercepts(0x3F8, 1) && !map.intercepts(0x3FF, 1));
    assert!(map.intercepts(0x400, 1));
    // A 4-byte access at 0x3FD covers 0x3FD-0x400: 0x400 still exits.
    assert!(!map.intercepts(0x3FC, 4));
    assert!(map.intercepts(0x3FD, 4));
    // An access at the top runs into the third page, which stays set.
    map.allow(0xFFFF, 1);
    assert!(!map.intercepts(0xFFFF, 1));
    assert!(map.intercepts(0xFFFF, 2));
    assert_eq!(map.0[0x3F8 / 8], 0x00);
}
