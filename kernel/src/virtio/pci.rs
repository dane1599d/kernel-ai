//! PCIe configuration access through the ECAM window from the MCFG table, and the
//! one pass over bus 0 that finds the virtio devices: the network card and the
//! block devices.

use super::hal::{DEVICE, KernelHal};
use crate::{log, memory};
use alloc::vec::Vec;
use virtio_drivers::transport::DeviceType;
use virtio_drivers::transport::pci::bus::{Command, ConfigurationAccess, DeviceFunction, PciRoot};
use virtio_drivers::transport::pci::{PciTransport, virtio_device_type};

/// Configuration space for bus `start_bus` onwards, `buses` buses mapped.
#[derive(Clone, Copy)]
pub struct EcamAccess {
    base: u64,
    start_bus: u8,
    buses: u16,
}

impl EcamAccess {
    fn address(&self, function: DeviceFunction, offset: u8) -> Option<u64> {
        let bus = function.bus.checked_sub(self.start_bus)?;
        if u16::from(bus) >= self.buses || function.device >= 32 || function.function >= 8 {
            return None;
        }
        Some(
            self.base
                + ((u64::from(bus) << 20)
                    | (u64::from(function.device) << 15)
                    | (u64::from(function.function) << 12)
                    | u64::from(offset & 0xFC)),
        )
    }
}

impl ConfigurationAccess for EcamAccess {
    fn read_word(&self, function: DeviceFunction, offset: u8) -> u32 {
        match self.address(function, offset) {
            // SAFETY: the address lies in the uncached ECAM window mapped by `probe`.
            Some(address) => unsafe { (address as *const u32).read_volatile() },
            None => u32::MAX,
        }
    }

    fn write_word(&mut self, function: DeviceFunction, offset: u8, data: u32) {
        if let Some(address) = self.address(function, offset) {
            // SAFETY: as in read_word.
            unsafe { (address as *mut u32).write_volatile(data) }
        }
    }

    unsafe fn unsafe_clone(&self) -> Self {
        *self
    }
}

/// The virtio devices on bus 0, each with its transport set up.
pub struct Found {
    /// The first network card; any other is left alone.
    pub net: Option<PciTransport>,
    /// A network card was there, but its transport could not be set up.
    pub net_failed: bool,
    /// Every block device, in bus order.
    pub disks: Vec<PciTransport>,
}

/// Enumerates bus 0 once and sets up a transport for each virtio network card and
/// block device on it.
pub fn probe(platform: &acpitables::Platform) -> Result<Found, &'static str> {
    let ecam = platform
        .ecam
        .iter()
        .find(|window| window.segment == 0)
        .ok_or("pci: no PCIe configuration window (MCFG) for segment 0")?;
    // On q35 the virtio devices sit on the root bus; one bus is 1 MiB of ECAM. The
    // MCFG base is where bus 0 would be, even if the window starts at a later bus.
    let buses = 1;
    let first_bus = ecam.base + (u64::from(ecam.start_bus) << 20);
    let base = memory::map_physical(first_bus, u64::from(buses) << 20, DEVICE)?;
    let mut root = PciRoot::new(EcamAccess {
        base,
        start_bus: ecam.start_bus,
        buses,
    });
    let wanted: Vec<(DeviceFunction, DeviceType, u16)> = root
        .enumerate_bus(ecam.start_bus)
        .filter_map(|(function, info)| match virtio_device_type(&info) {
            Some(kind @ (DeviceType::Network | DeviceType::Block)) => {
                Some((function, kind, info.device_id))
            }
            _ => None,
        })
        .collect();
    let mut found = Found {
        net: None,
        net_failed: false,
        disks: Vec::new(),
    };
    for (function, kind, device_id) in wanted {
        let name = match kind {
            DeviceType::Network if found.net.is_some() => {
                log!("pci: ignoring a second virtio network card at {function}");
                continue;
            }
            DeviceType::Network => "virtio-net",
            _ => "virtio-blk",
        };
        log!("pci: {name} at {function} (id 1af4:{device_id:04x})");
        // Firmware assigned the BARs; make sure the device decodes them and may DMA.
        let (_, command) = root.get_status_command(function);
        root.set_command(
            function,
            command | Command::MEMORY_SPACE | Command::BUS_MASTER,
        );
        let Ok(transport) = PciTransport::new::<KernelHal, _>(&mut root, function) else {
            log!("pci: {name} at {function}: transport setup failed");
            found.net_failed |= kind == DeviceType::Network;
            continue;
        };
        match kind {
            DeviceType::Network => found.net = Some(transport),
            _ => found.disks.push(transport),
        }
    }
    Ok(found)
}
