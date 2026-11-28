use super::*;
use alloc::vec;
use alloc::vec::Vec;

const COMMAND_OFF: u16 = 0x04;

/// A zeroed space of `len` bytes with Status bit 4 (a capability list) set.
fn space(len: usize) -> Vec<u8> {
    let mut s = vec![0u8; len];
    s[STATUS] = 0x10;
    s
}

fn put(space: &mut [u8], at: usize, bytes: &[u8]) {
    space[at..at + bytes.len()].copy_from_slice(bytes);
}

/// QEMU's virtio-net: vendor capabilities, then MSI-X at 0x98.
fn virtio_net() -> Vec<u8> {
    let mut s = space(256);
    put(&mut s, 0x04, &[0x07, 0x00]);
    s[0x34] = 0x98;
    put(&mut s, 0x98, &[0x11, 0x84, 0x03, 0x80]); // MSI-X, 4 vectors, enabled
    put(&mut s, 0x84, &[0x09, 0x70, 0x14, 0x04]); // vendor
    put(&mut s, 0x70, &[0x09, 0x60, 0x14, 0x03]);
    put(&mut s, 0x60, &[0x09, 0x50, 0x14, 0x02]);
    put(&mut s, 0x50, &[0x09, 0x40, 0x14, 0x01]);
    put(&mut s, 0x40, &[0x09, 0x00, 0x14, 0x05]);
    s
}

/// QEMU's nvme: power management, MSI (64-bit, masking), MSI-X, PCIe; and a
/// PCIe extended space with AER, then SR-IOV.
fn nvme() -> Vec<u8> {
    let mut s = space(4096);
    s[0x34] = 0x40;
    put(&mut s, 0x40, &[0x01, 0x50, 0x03, 0x00]);
    put(&mut s, 0x50, &[0x05, 0x60, 0x80 | 0x01, 0x01]); // MSI: 64-bit and masking
    put(&mut s, 0x60, &[0x11, 0x70, 0x1F, 0x00]);
    put(&mut s, 0x70, &[0x10, 0x00, 0x02, 0x00]);
    // AER at 0x100 (ID 1, version 2) -> SR-IOV at 0x150 -> end.
    put(
        &mut s,
        0x100,
        &(0x0001u32 | 2 << 16 | 0x150 << 20).to_le_bytes(),
    );
    put(&mut s, 0x150, &(0x0010u32 | 1 << 16).to_le_bytes());
    s
}

fn ok(s: &[u8], offset: u16, width: u8, value: u32) {
    assert_eq!(
        allow_write(s, offset, width, value),
        Ok(()),
        "{offset:#x}/{width}"
    );
}

fn refused(s: &[u8], offset: u16, width: u8, value: u32, why: Refused) {
    assert_eq!(
        allow_write(s, offset, width, value),
        Err(why),
        "{offset:#x}/{width}"
    );
}

#[test]
fn the_command_register_changes_only_memory_space_and_bus_master() {
    let s = virtio_net(); // command 0x0007: I/O, memory, bus master
    ok(&s, COMMAND_OFF, 2, 0x0007); // the same value
    ok(&s, COMMAND_OFF, 2, 0x0001); // memory and bus master off
    ok(&s, COMMAND_OFF, 2, 0x0005);
    ok(&s, COMMAND_OFF, 1, 0x01);
    ok(&s, COMMAND_OFF + 1, 1, 0x00);
    refused(&s, COMMAND_OFF, 2, 0x0006, Refused::CommandBits); // I/O space off
    refused(&s, COMMAND_OFF, 2, 0x0407, Refused::CommandBits); // Interrupt Disable on
    refused(&s, COMMAND_OFF, 2, 0x0107, Refused::CommandBits); // SERR on
    refused(&s, COMMAND_OFF + 1, 1, 0x04, Refused::CommandBits);
    refused(&s, COMMAND_OFF, 1, 0x00, Refused::CommandBits); // I/O off
    // Interrupt Disable already set by the host stays set when written back.
    let mut s = virtio_net();
    s[5] = 0x04;
    ok(&s, COMMAND_OFF, 2, 0x0407);
    refused(&s, COMMAND_OFF, 2, 0x0007, Refused::CommandBits);
}

#[test]
fn nothing_else_of_the_header_is_written() {
    let s = virtio_net();
    // Command and status together, status alone, the BARs, ROM BAR, the
    // capability pointer, the interrupt line, and the identity words.
    refused(&s, COMMAND_OFF, 4, 0x0000_0007, Refused::Header);
    refused(&s, 0x06, 2, 0, Refused::Header);
    for offset in (0x00u16..0x40).step_by(4).filter(|&o| o != COMMAND_OFF) {
        refused(&s, offset, 4, 0, Refused::Header);
    }
    for offset in 0x10..0x28 {
        refused(&s, offset, 1, 0, Refused::Header);
    }
    for offset in [0x2C, 0x30, 0x34, 0x3C] {
        refused(&s, offset, 4, 0, Refused::Header);
    }
    // Type 1: the bridge windows are in the header too.
    refused(&s, 0x20, 4, 0xFFFF_FFFF, Refused::Header);
    refused(&s, 0x24, 2, 0xFFFF, Refused::Header);
}

#[test]
fn the_width_alignment_extent_and_size_are_checked() {
    let s = virtio_net();
    for width in [0u8, 3, 5, 8, 255] {
        refused(&s, 0x44, width, 0, Refused::Width);
    }
    refused(&s, 0x45, 2, 0, Refused::Misaligned);
    refused(&s, 0x41, 4, 0, Refused::Misaligned);
    refused(&s, 0x102, 4, 0, Refused::Misaligned);
    refused(&s, 0xFE, 4, 0, Refused::Misaligned);
    refused(&s, 0x100, 4, 0, Refused::Outside);
    refused(&s, 0xFFFF, 1, 0, Refused::Outside);
    ok(&s, 0xFF, 1, 0); // device specific
    for len in [0, 255, 257, 1024, 4095, 4097] {
        refused(&vec![0; len], 0x44, 1, 0, Refused::SpaceSize);
    }
}

#[test]
fn msix_is_refused_whole_and_its_neighbours_are_not() {
    let s = virtio_net(); // MSI-X at 0x98, 12 bytes
    for offset in 0x98..0xA4 {
        refused(&s, offset, 1, 0, Refused::Capability);
    }
    refused(&s, 0x9C, 4, 0, Refused::Capability);
    ok(&s, 0xA4, 4, 0); // after it
    ok(&s, 0x94, 4, 0); // before it, inside the vendor capability at 0x84
    // The vendor capability's own ID and next bytes are refused, its body is not.
    refused(&s, 0x84, 2, 0, Refused::Capability);
    refused(&s, 0x85, 1, 0, Refused::Capability);
    ok(&s, 0x86, 2, 0);
    ok(&s, 0x88, 4, 0);
}

#[test]
fn msi_is_sized_by_its_message_control() {
    for (control, bytes) in [(0x0000u16, 12), (0x0080, 16), (0x0100, 20), (0x0180, 24)] {
        let mut s = space(256);
        s[0x34] = 0x50;
        put(&mut s, 0x50, &[0x05, 0x00]);
        put(&mut s, 0x52, &control.to_le_bytes());
        for offset in 0x50..0x50 + bytes {
            refused(&s, offset, 1, 0, Refused::Capability);
        }
        ok(&s, 0x50 + bytes, 1, 0);
        ok(&s, 0x50 + bytes + (4 - bytes % 4) % 4 + 4, 4, 0);
    }
}

#[test]
fn a_function_with_no_list_has_nothing_to_protect_above_the_header() {
    let mut s = virtio_net();
    s[STATUS] = 0;
    ok(&s, 0x98, 4, 0);
    ok(&s, 0xFC, 4, 0);
}

#[test]
fn a_malformed_standard_list_refuses_every_write_from_0x40_up() {
    let bad: [(&str, Vec<u8>); 3] = [
        ("a pointer below 0x40", {
            let mut s = space(256);
            s[0x34] = 0x30;
            s
        }),
        ("a loop", {
            let mut s = space(256);
            s[0x34] = 0x40;
            put(&mut s, 0x40, &[0x09, 0x44]);
            put(&mut s, 0x44, &[0x09, 0x40]);
            s
        }),
        ("a pointer to itself", {
            let mut s = space(256);
            s[0x34] = 0x40;
            put(&mut s, 0x40, &[0x09, 0x40]);
            s
        }),
    ];
    for (what, s) in bad {
        for offset in [0x40, 0x44, 0x80, 0xFC] {
            refused(&s, offset, 4, 0, Refused::BadList);
        }
        // Extended space too, once there is some.
        let mut wide = s.clone();
        wide.resize(4096, 0);
        refused(&wide, 0x100, 4, 0, Refused::BadList);
        refused(&wide, 0x800, 4, 0, Refused::BadList);
        // The header's command register still works.
        ok(&s, COMMAND_OFF, 2, 0x0006);
        let _ = what;
    }
    // 0x40 to 0xFC is 48 dwords, so a list that does not loop has at most 48.
    let mut s = space(256);
    s[0x34] = 0x40;
    for i in 0..48usize {
        let at = 0x40 + 4 * i;
        let next = if i == 47 { 0 } else { at + 4 };
        put(&mut s, at, &[0x09, next as u8]);
    }
    ok(&s, 0x42, 2, 0);
    refused(&s, 0x40, 2, 0, Refused::Capability);
}

#[test]
fn a_pointer_is_masked_to_a_dword() {
    let mut s = space(256);
    s[0x34] = 0x43; // reads as 0x40
    put(&mut s, 0x40, &[0x11, 0x00]);
    refused(&s, 0x4B, 1, 0, Refused::Capability);
}

#[test]
fn nvme_has_its_msi_msix_and_extended_capabilities_refused() {
    let s = nvme();
    ok(&s, 0x44, 4, 0); // power management control
    refused(&s, 0x50, 4, 0, Refused::Capability); // MSI header
    refused(&s, 0x5C, 4, 0, Refused::Capability); // MSI mask bits (24 bytes)
    refused(&s, 0x68, 4, 0, Refused::Capability); // MSI-X
    ok(&s, 0x6C, 4, 0); // just after it
    ok(&s, 0x74, 4, 0); // PCIe capability body
    refused(&s, 0x100, 4, 0, Refused::Capability); // AER header
    ok(&s, 0x104, 4, 0); // AER body (status registers)
    refused(&s, 0x150, 4, 0, Refused::Capability); // SR-IOV header
    for offset in (0x154..0x190).step_by(4) {
        refused(&s, offset, 4, 0, Refused::Capability);
    }
    ok(&s, 0x190, 4, 0); // past SR-IOV's 0x40 bytes
}

#[test]
fn bar_movers_are_refused_for_their_spec_length_whatever_follows() {
    // SR-IOV is 0x40 bytes, Resizable BAR and VF Resizable BAR 0x34.
    for (id, bytes) in [(0x0010u32, 0x40usize), (0x0015, 0x34), (0x0024, 0x34)] {
        let mut s = space(4096);
        put(&mut s, 0x100, &(id | 1 << 16 | 0x200 << 20).to_le_bytes());
        put(&mut s, 0x200, &(0x0001u32 | 1 << 16).to_le_bytes());
        for offset in (0x100..0x100 + bytes as u16).step_by(4) {
            refused(&s, offset, 4, 0, Refused::Capability);
        }
        ok(&s, 0x100 + bytes as u16, 4, 0);
        refused(&s, 0x200, 4, 0, Refused::Capability);
        ok(&s, 0x204, 4, 0);
    }
    // At the end of the space the body is clipped there, not past it.
    let mut s = space(4096);
    put(&mut s, 0x100, &(0x0001u32 | 0xFE0 << 20).to_le_bytes());
    put(&mut s, 0xFE0, &(0x0010u32 | 1 << 16).to_le_bytes());
    refused(&s, 0xFFC, 4, 0, Refused::Capability);
    ok(&s, 0xFDC, 4, 0);
}

#[test]
fn a_capability_that_starts_inside_a_bar_movers_body_is_a_malformed_list() {
    // The review's layout: SR-IOV at 0x100 whose next is 0x104.
    for (id, inside) in [
        (0x0010u32, 0x104usize),
        (0x0010, 0x13C),
        (0x0015, 0x104),
        (0x0024, 0x130),
    ] {
        let mut s = space(4096);
        put(
            &mut s,
            0x100,
            &(id | 1 << 16 | (inside as u32) << 20).to_le_bytes(),
        );
        put(&mut s, inside, &1u32.to_le_bytes());
        for offset in [0x108, 0x124, 0x104, 0x100, 0x200] {
            refused(&s, offset, 4, 0xFFFF_FFFF, Refused::BadExtended);
        }
        refused(&s, 0x108, 2, 0x0009, Refused::BadExtended);
        ok(&s, 0x44, 4, 0); // the standard space is unaffected
    }
    // Just past the body is fine.
    let mut s = space(4096);
    put(
        &mut s,
        0x100,
        &(0x0010u32 | 1 << 16 | 0x140 << 20).to_le_bytes(),
    );
    put(&mut s, 0x140, &1u32.to_le_bytes());
    refused(&s, 0x124, 4, 0, Refused::Capability);
    refused(&s, 0x140, 4, 0, Refused::Capability);
}

#[test]
fn a_multicast_capability_refuses_every_extended_write() {
    let mut s = space(4096);
    put(&mut s, 0x100, &(0x0001u32 | 0x140 << 20).to_le_bytes());
    put(&mut s, 0x140, &(0x0012u32 | 1 << 16).to_le_bytes());
    refused(&s, 0x200, 4, 0, Refused::BadExtended);
    ok(&s, 0x44, 4, 0);
}

#[test]
fn an_msi_or_msix_body_past_0xff_is_a_malformed_standard_list() {
    // The review's layouts: MSI at 0xF0 (24 bytes), 0xF4 and 0xF8 (64-bit),
    // MSI-X at 0xF8 and 0xFC; with a zero and with an AER dword at 0x100.
    let msi = |control: u16| [0x05, 0x00, control as u8, (control >> 8) as u8];
    let cases: [(usize, [u8; 4]); 7] = [
        (0xF0, msi(0x0180)),
        (0xF4, msi(0x0080)),
        (0xF8, msi(0x0080)),
        (0xF8, msi(0x0000)),
        (0xFC, msi(0x0000)),
        (0xF8, [0x11, 0, 0, 0]),
        (0xFC, [0x11, 0, 0, 0]),
    ];
    for (at, cap) in cases {
        for dword in [0u32, 0x0001 | 2 << 16] {
            let mut s = space(4096);
            s[0x34] = at as u8;
            put(&mut s, at, &cap);
            put(&mut s, 0x100, &dword.to_le_bytes());
            for offset in [0x100, 0x102, 0x104, 0x40, 0xF0, 0xFC, 0x800] {
                refused(&s, offset, 2, 0, Refused::BadList);
            }
            ok(&s, COMMAND_OFF, 2, 0x0006);
        }
    }
    // A body that ends exactly at 0x100 is fine: MSI 64-bit at 0xF0 is 16 bytes.
    let mut s = space(4096);
    s[0x34] = 0xF0;
    put(&mut s, 0xF0, &msi(0x0080));
    refused(&s, 0xFE, 2, 0, Refused::Capability);
    ok(&s, 0x100, 4, 0);
}

#[test]
fn enhanced_allocation_and_hypertransport_refuse_everything_from_0x40() {
    for id in [0x14u8, 0x08] {
        let mut s = space(256);
        s[0x34] = 0x60;
        put(&mut s, 0x60, &[id, 0x00]);
        refused(&s, 0x44, 4, 0, Refused::BadList);
        refused(&s, 0xFC, 4, 0, Refused::BadList);
    }
}

#[test]
fn a_header_type_other_than_0_or_1_refuses_everything_but_the_command_register() {
    for kind in [2u8, 3, 0x7F, 0x82] {
        let mut s = virtio_net();
        s[0x0E] = kind;
        refused(&s, 0x44, 4, 0, Refused::HeaderType);
        refused(&s, 0xFC, 4, 0, Refused::HeaderType);
        ok(&s, COMMAND_OFF, 2, 0x0007);
    }
    // The multifunction bit (0x80) is not a type.
    for kind in [0x80u8, 0x81] {
        let mut s = virtio_net();
        s[0x0E] = kind;
        ok(&s, 0x44, 4, 0);
    }
}

#[test]
fn value_bits_above_the_width_are_refused() {
    let s = virtio_net();
    refused(&s, 0x44, 1, 0x100, Refused::Value);
    refused(&s, 0x44, 2, 0x1_0000, Refused::Value);
    refused(&s, COMMAND_OFF, 1, 0xFFFF_FF06, Refused::Value);
    ok(&s, 0x44, 1, 0xFF);
    ok(&s, 0x44, 2, 0xFFFF);
    ok(&s, 0x44, 4, 0xFFFF_FFFF);
}

#[test]
fn a_malformed_extended_list_refuses_the_extended_writes_only() {
    let bad: [Vec<u8>; 4] = [
        {
            // next below 0x100
            let mut s = space(4096);
            put(&mut s, 0x100, &(0x0001u32 | 0x0F0 << 20).to_le_bytes());
            s
        },
        {
            // a loop
            let mut s = space(4096);
            put(&mut s, 0x100, &(0x0001u32 | 0x200 << 20).to_le_bytes());
            put(&mut s, 0x200, &(0x0001u32 | 0x100 << 20).to_le_bytes());
            s
        },
        {
            // all ones where a device is gone
            let mut s = space(4096);
            put(&mut s, 0x100, &[0xFF; 4]);
            s
        },
        {
            // a next that is not dword aligned reads masked: 0x103 -> 0x100, a loop
            let mut s = space(4096);
            put(&mut s, 0x100, &(0x0001u32 | 0x103 << 20).to_le_bytes());
            s
        },
    ];
    for s in bad {
        for offset in [0x100, 0x104, 0x200, 0xFFC] {
            refused(&s, offset, 4, 0, Refused::BadExtended);
        }
        ok(&s, 0x44, 4, 0); // the standard space is unaffected
        ok(&s, COMMAND_OFF, 2, 0);
    }
    // No list (a zero first dword) is fine.
    ok(&space(4096), 0x100, 4, 0);
    ok(&space(4096), 0xFFC, 4, 0);
}

#[test]
fn the_extended_list_is_bounded_at_960_entries() {
    // 960 dwords from 0x100, each pointing at the next: the most there are.
    let mut s = space(4096);
    for i in 0..MAX_EXTENDED {
        let at = 0x100 + 4 * i;
        let next = if i + 1 == MAX_EXTENDED { 0 } else { at + 4 };
        put(&mut s, at, &(0x0001u32 | (next as u32) << 20).to_le_bytes());
    }
    refused(&s, 0x100, 4, 0, Refused::Capability);
    refused(&s, 0xFFC, 4, 0, Refused::Capability);
}

#[test]
fn a_256_byte_space_has_no_extended_space_to_write() {
    let s = virtio_net();
    refused(&s, 0x100, 4, 0, Refused::Outside);
}
