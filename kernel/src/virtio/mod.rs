//! Virtio devices on PCI: the DMA layer the drivers share, the one pass over the
//! bus that finds them, and the read-only block driver. The network card's own
//! code is in `net` and the GPU's in `display`; the same pass finds the NVMe
//! controllers for `nvme` and the xHCI controllers for `usb`.

pub mod blk;
pub mod hal;
mod pci;
pub mod split;

use crate::log;

/// Finds the PCI devices and brings up the network card, the disks and the GPU,
/// which becomes the console's display. A device that is missing or fails is
/// logged, and the kernel runs on without it (on GOP, for the GPU).
pub fn init(platform: &acpitables::Platform, cmdline: &str) {
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
    if let Some(transport) = found.gpu {
        match crate::display::init_gpu(transport, cmdline) {
            Ok(surface) => {
                crate::console::attach(surface);
            }
            Err(message) => log!("{message}; staying on GOP"),
        }
    }
    blk::init(found.disks);
    crate::nvme::init(found.nvme);
    crate::usb::init(found.usb);
}
