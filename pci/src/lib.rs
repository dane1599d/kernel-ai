//! The PCI walk (ADR 0007 section 3), on `pci_types`: every bus reachable from a
//! segment's first bus, following bridges through the secondary bus numbers
//! firmware set. BARs stay as firmware assigned them; nothing here writes
//! configuration space. The kernel supplies the configuration access (ECAM,
//! mapping each bus as the walk first reads it); host tests use a fake one.
//! [`msix`] reads capabilities and sets up MSI-X.

#![no_std]

extern crate alloc;

pub mod msix;

use alloc::vec::Vec;
pub use pci_types::{ConfigRegionAccess, PciAddress};
use pci_types::{HeaderType, PciHeader, PciPciBridgeHeader};

/// Functions the walk returns at most: far more than any machine we target, and a
/// bound on what a broken or hostile configuration space can make it do.
pub const MAX_FUNCTIONS: usize = 1024;

/// One function the walk found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Function {
    pub address: PciAddress,
    pub vendor: u16,
    pub device: u16,
    /// Class, subclass and programming interface.
    pub class: (u8, u8, u8),
    pub revision: u8,
    /// For a PCI-to-PCI bridge, the secondary bus firmware gave it.
    pub bridge_to: Option<u8>,
}

/// Every function on `start_bus` and on the buses behind its bridges, as long
/// as they lie within `start_bus..=end_bus` (the segment's MCFG range), in
/// bus, device and function order. A bus is visited once, so a bridge that
/// points back cannot loop the walk.
pub fn walk(
    access: &impl ConfigRegionAccess,
    segment: u16,
    start_bus: u8,
    end_bus: u8,
) -> Vec<Function> {
    let mut found = Vec::new();
    let mut visited = [false; 256];
    let mut pending = Vec::from([start_bus]);
    while let Some(bus) = pending.pop() {
        if visited[usize::from(bus)] || bus < start_bus || bus > end_bus {
            continue;
        }
        visited[usize::from(bus)] = true;
        for device in 0..32 {
            let first = PciAddress::new(segment, bus, device, 0);
            let header = PciHeader::new(first);
            if header.id(access).0 == 0xFFFF {
                continue;
            }
            let functions = if header.has_multiple_functions(access) {
                8
            } else {
                1
            };
            for function in 0..functions {
                if found.len() == MAX_FUNCTIONS {
                    return sorted(found);
                }
                let address = PciAddress::new(segment, bus, device, function);
                let Some(f) = read(access, address) else {
                    continue;
                };
                if let Some(secondary) = f.bridge_to.filter(|s| *s > bus) {
                    pending.push(secondary);
                }
                found.push(f);
            }
        }
    }
    sorted(found)
}

fn sorted(mut found: Vec<Function>) -> Vec<Function> {
    found.sort_by_key(|f| (f.address.bus(), f.address.device(), f.address.function()));
    found
}

/// The function at `address`, if one answers there.
fn read(access: &impl ConfigRegionAccess, address: PciAddress) -> Option<Function> {
    let header = PciHeader::new(address);
    let (vendor, device) = header.id(access);
    if vendor == 0xFFFF {
        return None;
    }
    let (revision, base, sub, interface) = header.revision_and_class(access);
    let bridge_to = match header.header_type(access) {
        HeaderType::PciPciBridge => PciPciBridgeHeader::from_header(header, access)
            .map(|bridge| bridge.secondary_bus_number(access)),
        _ => None,
    };
    Some(Function {
        address,
        vendor,
        device,
        class: (base, sub, interface),
        revision,
        bridge_to,
    })
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::collections::BTreeMap;

    /// Configuration space as a map from (bus, device, function) to its first
    /// 64 bytes; anything absent reads as all ones.
    #[derive(Default)]
    struct Fake(BTreeMap<(u8, u8, u8), [u32; 16]>);

    impl Fake {
        fn add(&mut self, at: (u8, u8, u8), id: (u16, u16), class: (u8, u8, u8), kind: u8) {
            let mut space = [0u32; 16];
            space[0] = u32::from(id.0) | (u32::from(id.1) << 16);
            space[2] = (u32::from(class.0) << 24)
                | (u32::from(class.1) << 16)
                | (u32::from(class.2) << 8)
                | 1;
            space[3] = u32::from(kind) << 16;
            self.0.insert(at, space);
        }

        fn endpoint(&mut self, at: (u8, u8, u8), id: (u16, u16), class: (u8, u8, u8)) {
            self.add(at, id, class, 0);
        }

        /// A bridge whose secondary bus is `to`.
        fn bridge(&mut self, at: (u8, u8, u8), to: u8) {
            self.add(at, (0x1b36, 0x000c), (0x06, 0x04, 0x00), 1);
            let space = self.0.get_mut(&at).unwrap();
            space[6] = u32::from(at.0) | (u32::from(to) << 8) | (u32::from(to) << 16);
        }

        /// Marks function 0 of a device as having several functions.
        fn multi(&mut self, at: (u8, u8)) {
            self.0.get_mut(&(at.0, at.1, 0)).unwrap()[3] |= 0x80 << 16;
        }
    }

    impl ConfigRegionAccess for Fake {
        unsafe fn read(&self, address: PciAddress, offset: u16) -> u32 {
            let at = (address.bus(), address.device(), address.function());
            self.0
                .get(&at)
                .and_then(|space| space.get(usize::from(offset / 4)).copied())
                .unwrap_or(u32::MAX)
        }

        unsafe fn write(&self, _: PciAddress, _: u16, _: u32) {
            panic!("the walk never writes configuration space");
        }
    }

    fn places(found: &[Function]) -> Vec<(u8, u8, u8)> {
        found
            .iter()
            .map(|f| (f.address.bus(), f.address.device(), f.address.function()))
            .collect()
    }

    #[test]
    fn devices_behind_bridges_are_found_in_bus_order() {
        let mut fake = Fake::default();
        fake.endpoint((0, 0, 0), (0x8086, 0x29c0), (0x06, 0x00, 0x00));
        fake.bridge((0, 2, 0), 1);
        fake.bridge((0, 3, 0), 2);
        fake.endpoint((1, 0, 0), (0x1af4, 0x1041), (0x02, 0x00, 0x00));
        fake.bridge((2, 0, 0), 3);
        fake.endpoint((3, 0, 0), (0x1b36, 0x0010), (0x01, 0x08, 0x02));
        let found = walk(&fake, 0, 0, 255);
        assert_eq!(
            places(&found),
            [
                (0, 0, 0),
                (0, 2, 0),
                (0, 3, 0),
                (1, 0, 0),
                (2, 0, 0),
                (3, 0, 0)
            ]
        );
        assert_eq!(found[1].bridge_to, Some(1));
        let net = found[3];
        assert_eq!(
            (net.vendor, net.device, net.class),
            (0x1af4, 0x1041, (2, 0, 0))
        );
        assert_eq!(found[5].class, (1, 8, 2));
    }

    #[test]
    fn functions_beyond_zero_are_read_only_on_multi_function_devices() {
        let mut fake = Fake::default();
        fake.endpoint((0, 1, 0), (0x8086, 0x2918), (0x06, 0x01, 0x00));
        fake.endpoint((0, 1, 3), (0x8086, 0x2930), (0x0c, 0x05, 0x00));
        fake.endpoint((0, 2, 0), (0x1af4, 0x1042), (0x01, 0x00, 0x00));
        fake.endpoint((0, 2, 1), (0x1af4, 0x1042), (0x01, 0x00, 0x00));
        fake.multi((0, 1));
        assert_eq!(
            places(&walk(&fake, 0, 0, 255)),
            [(0, 1, 0), (0, 1, 3), (0, 2, 0)]
        );
    }

    #[test]
    fn bridges_that_point_back_or_out_of_range_do_not_loop_or_escape() {
        let mut fake = Fake::default();
        fake.bridge((0, 1, 0), 0); // to itself
        fake.bridge((0, 2, 0), 9); // past the MCFG range
        fake.bridge((0, 3, 0), 1);
        fake.bridge((1, 0, 0), 0); // back to the root
        fake.endpoint((9, 0, 0), (0x1af4, 0x1041), (0x02, 0x00, 0x00));
        let found = walk(&fake, 0, 0, 4);
        assert_eq!(places(&found), [(0, 1, 0), (0, 2, 0), (0, 3, 0), (1, 0, 0)]);
    }

    #[test]
    fn a_window_starting_past_bus_zero_is_walked_from_its_first_bus() {
        let mut fake = Fake::default();
        fake.endpoint((0, 0, 0), (0x8086, 0x29c0), (0x06, 0x00, 0x00));
        fake.endpoint((4, 0, 0), (0x1af4, 0x1041), (0x02, 0x00, 0x00));
        assert_eq!(places(&walk(&fake, 0, 4, 8)), [(4, 0, 0)]);
        assert_eq!(walk(&fake, 0, 4, 8)[0].address.segment(), 0);
    }

    #[test]
    fn a_configuration_space_that_answers_everywhere_is_bounded() {
        struct Everywhere;
        impl ConfigRegionAccess for Everywhere {
            unsafe fn read(&self, address: PciAddress, offset: u16) -> u32 {
                match offset {
                    0x0c => 0x0081_0000, // a multi-function bridge
                    0x18 => u32::from(address.bus()).wrapping_add(1) << 8,
                    _ => 0x1234_5678,
                }
            }
            unsafe fn write(&self, _: PciAddress, _: u16, _: u32) {}
        }
        assert_eq!(walk(&Everywhere, 0, 0, 255).len(), MAX_FUNCTIONS);
    }
}
