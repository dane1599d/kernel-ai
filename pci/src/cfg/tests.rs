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
    for (control, bytes) in [(0x0000u16, 10), (0x0080, 14), (0x0100, 20), (0x0180, 24)] {
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
    for offset in (0x154..0x1000).step_by(4) {
        refused(&s, offset, 4, 0, Refused::Capability);
    }
}

#[test]
fn resizable_bars_stop_at_the_next_capability() {
    for id in [0x0015u32, 0x0024] {
        let mut s = space(4096);
        put(&mut s, 0x100, &(id | 1 << 16 | 0x180 << 20).to_le_bytes());
        put(&mut s, 0x180, &(0x0001u32 | 1 << 16).to_le_bytes());
        refused(&s, 0x100, 4, 0, Refused::Capability);
        refused(&s, 0x104, 4, 0, Refused::Capability);
        refused(&s, 0x17C, 4, 0, Refused::Capability);
        refused(&s, 0x180, 4, 0, Refused::Capability);
        ok(&s, 0x184, 4, 0);
    }
    // A next pointer that goes back to an earlier capability does not
    // shorten the body below the next one that starts after it.
    let mut s = space(4096);
    put(&mut s, 0x100, &(0x0001u32 | 0x200 << 20).to_le_bytes());
    put(&mut s, 0x200, &(0x0015u32 | 0x180 << 20).to_le_bytes());
    put(&mut s, 0x180, &(0x0001u32).to_le_bytes());
    refused(&s, 0x204, 4, 0, Refused::Capability);
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
