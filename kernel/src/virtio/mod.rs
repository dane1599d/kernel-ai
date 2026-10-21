//! Virtio devices on PCI: the DMA layer the drivers share, the one pass over the
//! bus that finds them, and the read-only block driver. The network card's own
//! code is in `net`; the same pass finds the NVMe controllers for `nvme`.

pub mod blk;
pub mod hal;
mod pci;
pub mod split;

use crate::log;

/// Finds the PCI devices and brings up the network card and the disks. A device
/// that is missing or fails is logged, and the kernel runs on without it.
pub fn init(platform: &acpitables::Platform) {
    let found = match pci::probe(platform) {
        Ok(found) => found,
        Err(message) => {
            log!("{message}");
            return;
        }
    };
    match found.net {
        Some(transport) => {
            if let Err(message) = crate::net::init(transport) {
                log!("{message}");
            }
        }
        None if found.net_failed => log!("net: the virtio network card did not set up"),
        None => log!("net: no virtio network card found"),
    }
    blk::init(found.disks);
    crate::nvme::init(found.nvme);
}
