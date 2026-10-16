//! PCIe configuration access through the ECAM window from the MCFG table, and the
//! probe that finds the virtio network card.

use super::hal::{DEVICE, KernelHal};
use crate::{log, memory};
use virtio_drivers::device::net::VirtIONet;
use virtio_drivers::transport::DeviceType;
use virtio_drivers::transport::pci::bus::{Command, ConfigurationAccess, DeviceFunction, PciRoot};
use virtio_drivers::transport::pci::{PciTransport, virtio_device_type};

/// Receive and transmit queue depth: a power of two, kept small because the driver
/// builds its queue arrays on the stack.
pub const QUEUE_SIZE: usize = 16;
/// Per-packet buffer: at least 1526 bytes and a multiple of 8.
pub const BUFFER_LEN: usize = 2048;

pub type Nic = VirtIONet<KernelHal, PciTransport, QUEUE_SIZE>;

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

/// Finds the first virtio network card on bus 0 and brings it up, polled.
pub fn probe(platform: &acpitables::Platform) -> Result<Nic, &'static str> {
    let ecam = platform
        .ecam
        .iter()
        .find(|window| window.segment == 0)
        .ok_or("net: no PCIe configuration window (MCFG) for segment 0")?;
    // On q35 the virtio card sits on the root bus; one bus is 1 MiB of ECAM. The
    // MCFG base is where bus 0 would be, even if the window starts at a later bus.
    let buses = 1;
    let first_bus = ecam.base + (u64::from(ecam.start_bus) << 20);
    let base = memory::map_physical(first_bus, u64::from(buses) << 20, DEVICE)?;
    let mut root = PciRoot::new(EcamAccess {
        base,
        start_bus: ecam.start_bus,
        buses,
    });
    for (function, info) in root.enumerate_bus(ecam.start_bus) {
        if virtio_device_type(&info) != Some(DeviceType::Network) {
            continue;
        }
        log!(
            "net: virtio-net at {function} (id {:04x}:{:04x})",
            info.vendor_id,
            info.device_id
        );
        // Firmware assigned the BARs; make sure the card decodes them and may DMA.
        let (_, command) = root.get_status_command(function);
        root.set_command(
            function,
            command | Command::MEMORY_SPACE | Command::BUS_MASTER,
        );
        let transport = PciTransport::new::<KernelHal, _>(&mut root, function)
            .map_err(|_| "net: virtio PCI transport setup failed")?;
        let mut nic =
            Nic::new(transport, BUFFER_LEN).map_err(|_| "net: virtio-net setup failed")?;
        nic.disable_interrupts();
        return Ok(nic);
    }
    Err("net: no virtio network card found")
}
