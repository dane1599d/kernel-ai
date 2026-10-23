//! Capabilities and MSI-X (ADR 0007 section 4): a bounded walk of a function's
//! capability list, its MSI-X capability, the address of the BAR its table sits
//! in (read, never sized, so a running device is not disturbed), and the
//! message a vector is delivered with.

use crate::{ConfigRegionAccess, PciAddress};
use alloc::vec::Vec;

/// Hops a capability list is followed for, as `split::common_cfg` does: a list
/// that loops or runs on is cut off here.
pub const MAX_CAPABILITY_HOPS: usize = 48;
/// The MSI-X capability's ID.
pub const CAP_MSIX: u8 = 0x11;
/// Status register bit: the function has a capability list.
const STATUS_CAPABILITIES: u32 = 1 << 20;
const CAPABILITY_POINTER: u16 = 0x34;
/// Message control: enable, and mask every vector of the function.
const MSIX_ENABLE: u32 = 1 << 31;
const MSIX_FUNCTION_MASK: u32 = 1 << 30;
/// Bytes of one MSI-X table entry: address low, address high, data, control.
pub const ENTRY_BYTES: u64 = 16;

fn read(access: &impl ConfigRegionAccess, address: PciAddress, offset: u16) -> u32 {
    // SAFETY: the caller's access covers this function's configuration space;
    // offsets here stay inside its first 256 bytes.
    unsafe { access.read(address, offset) }
}

fn write(access: &impl ConfigRegionAccess, address: PciAddress, offset: u16, value: u32) {
    // SAFETY: as in read.
    unsafe { access.write(address, offset, value) }
}

/// The function's capabilities as (ID, offset), in list order, at most
/// [`MAX_CAPABILITY_HOPS`] of them.
pub fn capabilities(access: &impl ConfigRegionAccess, address: PciAddress) -> Vec<(u8, u16)> {
    let mut found = Vec::new();
    if read(access, address, 0x04) & STATUS_CAPABILITIES == 0 {
        return found;
    }
    let mut at = (read(access, address, CAPABILITY_POINTER) & 0xFC) as u16;
    while at >= 0x40 && found.len() < MAX_CAPABILITY_HOPS {
        let header = read(access, address, at);
        found.push((header as u8, at));
        at = ((header >> 8) & 0xFC) as u16;
    }
    found
}

/// A function's MSI-X capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Msix {
    /// Where the capability sits in configuration space.
    pub offset: u16,
    /// Entries in the table.
    pub table_size: u16,
    pub table_bar: u8,
    pub table_offset: u32,
    pub pba_bar: u8,
    pub pba_offset: u32,
}

impl Msix {
    /// Turns MSI-X on, every vector unmasked at the function level (each entry
    /// keeps its own mask), or off.
    pub fn enable(&self, access: &impl ConfigRegionAccess, address: PciAddress, on: bool) {
        let control = read(access, address, self.offset) & !MSIX_FUNCTION_MASK;
        let control = match on {
            true => control | MSIX_ENABLE,
            false => control & !MSIX_ENABLE,
        };
        write(access, address, self.offset, control);
    }
}

/// The function's MSI-X capability, if it has one.
pub fn msix(access: &impl ConfigRegionAccess, address: PciAddress) -> Option<Msix> {
    let (_, offset) = capabilities(access, address)
        .into_iter()
        .find(|(id, _)| *id == CAP_MSIX)?;
    let control = read(access, address, offset) >> 16;
    let table = read(access, address, offset + 4);
    let pba = read(access, address, offset + 8);
    Some(Msix {
        offset,
        table_size: (control & 0x7FF) as u16 + 1,
        table_bar: (table & 0x7) as u8,
        table_offset: table & !0x7,
        pba_bar: (pba & 0x7) as u8,
        pba_offset: pba & !0x7,
    })
}

/// Where memory BAR `bar` (0-5) of an endpoint lies, from its registers alone:
/// `None` for an I/O BAR, a BAR firmware left at 0, or the upper half of a
/// 64-bit BAR.
pub fn bar_address(access: &impl ConfigRegionAccess, address: PciAddress, bar: u8) -> Option<u64> {
    if bar > 5 {
        return None;
    }
    let offset = 0x10 + 4 * u16::from(bar);
    let low = read(access, address, offset);
    if low & 1 != 0 {
        return None;
    }
    let base = u64::from(low & !0xF);
    let wide = (low >> 1) & 0b11 == 0b10;
    let high = match wide {
        true if bar < 5 => u64::from(read(access, address, offset + 4)) << 32,
        true => return None,
        false => 0,
    };
    Some(base | high).filter(|a| *a != 0)
}

/// The address and data that deliver `vector` to the local APIC with ID
/// `apic_id` (xAPIC, physical destination, fixed delivery, edge-triggered).
pub fn message(apic_id: u8, vector: u8) -> (u64, u32) {
    (0xFEE0_0000 | (u64::from(apic_id) << 12), u32::from(vector))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::cell::RefCell;
    use std::collections::BTreeMap;

    /// One function's configuration space by dword offset; absent reads as 0.
    #[derive(Default)]
    struct Space(RefCell<BTreeMap<u16, u32>>);

    impl Space {
        fn set(&self, offset: u16, value: u32) {
            self.0.borrow_mut().insert(offset, value);
        }
        fn get(&self, offset: u16) -> u32 {
            self.0.borrow().get(&offset).copied().unwrap_or(0)
        }
    }

    impl ConfigRegionAccess for Space {
        unsafe fn read(&self, _: PciAddress, offset: u16) -> u32 {
            self.get(offset)
        }
        unsafe fn write(&self, _: PciAddress, offset: u16, value: u32) {
            self.set(offset, value);
        }
    }

    const AT: fn() -> PciAddress = || PciAddress::new(0, 0, 4, 0);

    /// A function with power management at 0x40, MSI at 0x50 and MSI-X at 0x70:
    /// 16 entries, table in BAR 0 at 0x3000, PBA in BAR 0 at 0x3800.
    fn xhci_like() -> Space {
        let space = Space::default();
        space.set(0x04, STATUS_CAPABILITIES);
        space.set(CAPABILITY_POINTER, 0x40);
        space.set(0x40, 0x01 | (0x50 << 8));
        space.set(0x50, 0x05 | (0x70 << 8));
        space.set(0x70, u32::from(CAP_MSIX) | (15 << 16));
        space.set(0x74, 0x3000);
        space.set(0x78, 0x3800);
        space
    }

    #[test]
    fn the_capability_list_and_msix_are_read() {
        let space = xhci_like();
        assert_eq!(
            capabilities(&space, AT()),
            [(0x01, 0x40), (0x05, 0x50), (CAP_MSIX, 0x70)]
        );
        let msix = msix(&space, AT()).unwrap();
        assert_eq!(
            msix,
            Msix {
                offset: 0x70,
                table_size: 16,
                table_bar: 0,
                table_offset: 0x3000,
                pba_bar: 0,
                pba_offset: 0x3800,
            }
        );
    }

    #[test]
    fn enabling_msix_sets_enable_and_clears_the_function_mask() {
        let space = xhci_like();
        space.set(0x70, space.get(0x70) | MSIX_FUNCTION_MASK);
        let msix = msix(&space, AT()).unwrap();
        msix.enable(&space, AT(), true);
        let control = space.get(0x70);
        assert_eq!(control & (MSIX_ENABLE | MSIX_FUNCTION_MASK), MSIX_ENABLE);
        assert_eq!(control & 0xFFFF, u32::from(CAP_MSIX), "the header is kept");
        msix.enable(&space, AT(), false);
        assert_eq!(space.get(0x70) & MSIX_ENABLE, 0);
    }

    #[test]
    fn a_looping_or_missing_list_is_bounded() {
        let space = Space::default();
        assert!(capabilities(&space, AT()).is_empty(), "no list bit");
        space.set(0x04, STATUS_CAPABILITIES);
        space.set(CAPABILITY_POINTER, 0x40);
        space.set(0x40, 0x09 | (0x40 << 8)); // points at itself
        assert_eq!(capabilities(&space, AT()).len(), MAX_CAPABILITY_HOPS);
        assert_eq!(msix(&space, AT()), None);
        space.set(CAPABILITY_POINTER, 0x20); // into the header: not a capability
        assert!(capabilities(&space, AT()).is_empty());
    }

    #[test]
    fn bars_are_read_without_sizing() {
        let space = Space::default();
        space.set(0x10, 0xFEBF_0000); // 32-bit memory
        space.set(0x14, 0x0000_C001); // I/O
        space.set(0x18, 0x8000_000C); // 64-bit prefetchable memory, low half
        space.set(0x1C, 0x0000_0001); // its high half
        space.set(0x24, 0x0000_0004); // 64-bit in BAR 5 has no high half
        assert_eq!(bar_address(&space, AT(), 0), Some(0xFEBF_0000));
        assert_eq!(bar_address(&space, AT(), 1), None);
        assert_eq!(bar_address(&space, AT(), 2), Some(0x1_8000_0000));
        assert_eq!(bar_address(&space, AT(), 4), None, "unassigned");
        assert_eq!(bar_address(&space, AT(), 5), None);
        assert_eq!(bar_address(&space, AT(), 6), None);
        assert!(
            space.0.borrow().values().all(|v| *v != u32::MAX),
            "nothing was sized"
        );
    }

    #[test]
    fn a_message_targets_one_apic_with_its_vector() {
        assert_eq!(message(0, 0x30), (0xFEE0_0000, 0x30));
        assert_eq!(message(3, 0xEF), (0xFEE0_3000, 0xEF));
    }
}
