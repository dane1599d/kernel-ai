//! PCIe configuration access through the ECAM window from the MCFG table, and the
//! one pass over bus 0 that finds the virtio devices (the network card, the block
//! devices and the GPU), the NVMe controllers and the xHCI (USB 3) controllers.

use super::hal::{DEVICE, KernelHal};
use super::split::{self, SplitTransport};
use crate::{log, memory};
use alloc::format;
use alloc::vec::Vec;
use virtio_drivers::transport::DeviceType;
use virtio_drivers::transport::pci::bus::{
    BarInfo, Command, ConfigurationAccess, DeviceFunction, PciRoot,
};
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
    pub net: Option<SplitTransport>,
    /// A network card was there, but its transport could not be set up.
    pub net_failed: bool,
    /// Every block device, in bus order.
    pub disks: Vec<PciTransport>,
    /// The first virtio-gpu; any other is left alone.
    pub gpu: Option<SplitTransport>,
    /// Every NVMe controller, in bus order.
    pub nvme: Vec<crate::nvme::Function>,
    /// Every xHCI controller, in bus order.
    pub usb: Vec<crate::usb::Function>,
}

/// PCI class, subclass and programming interface of an NVMe controller.
const NVME_CLASS: (u8, u8, u8) = (0x01, 0x08, 0x02);
/// PCI class, subclass and programming interface of an xHCI controller.
const XHCI_CLASS: (u8, u8, u8) = (0x0C, 0x03, 0x30);

/// Enumerates bus 0 once and sets up a transport for each virtio network card,
/// block device and GPU on it.
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
    let access = EcamAccess {
        base,
        start_bus: ecam.start_bus,
        buses,
    };
    let mut root = PciRoot::new(access);
    let functions: Vec<_> = root.enumerate_bus(ecam.start_bus).collect();
    let of_class = |class| -> Vec<DeviceFunction> {
        functions
            .iter()
            .filter(|(_, info)| (info.class, info.subclass, info.prog_if) == class)
            .map(|&(function, _)| function)
            .collect()
    };
    let nvme = of_class(NVME_CLASS);
    let xhci = of_class(XHCI_CLASS);
    let wanted: Vec<(DeviceFunction, DeviceType, u16)> = functions
        .into_iter()
        .filter_map(|(function, info)| match virtio_device_type(&info) {
            Some(kind @ (DeviceType::Network | DeviceType::Block | DeviceType::GPU)) => {
                Some((function, kind, info.device_id))
            }
            _ => None,
        })
        .collect();
    let mut found = Found {
        net: None,
        net_failed: false,
        disks: Vec::new(),
        gpu: None,
        nvme: Vec::new(),
        usb: Vec::new(),
    };
    for (function, kind, device_id) in wanted {
        let name = match kind {
            DeviceType::Network if found.net.is_some() => {
                log!("pci: ignoring a second virtio network card at {function}");
                continue;
            }
            DeviceType::Network => "virtio-net",
            DeviceType::GPU if found.gpu.is_some() => {
                log!("pci: ignoring a second virtio-gpu at {function}");
                continue;
            }
            DeviceType::GPU => "virtio-gpu",
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
            DeviceType::Network => match split_transport(&access, function, transport) {
                Ok(transport) => found.net = Some(transport),
                Err(message) => {
                    log!("pci: {name} at {function}: {message}");
                    found.net_failed = true;
                }
            },
            DeviceType::GPU => match split_transport(&access, function, transport) {
                Ok(transport) => found.gpu = Some(transport),
                Err(message) => log!("pci: {name} at {function}: {message}"),
            },
            _ => found.disks.push(transport),
        }
    }
    for function in nvme {
        match memory_bar0(&mut root, function) {
            Ok((bar, bar_len)) => {
                log!(
                    "pci: nvme at {function}, BAR0 {bar:#x} ({} KiB)",
                    bar_len / 1024
                );
                found.nvme.push(crate::nvme::Function {
                    name: format!("{function}"),
                    bar,
                    bar_len,
                });
            }
            Err(message) => log!("pci: nvme at {function}: {message}"),
        }
    }
    for function in xhci {
        match memory_bar0(&mut root, function) {
            Ok((bar, bar_len)) => {
                log!(
                    "pci: xhci at {function}, BAR0 {bar:#x} ({} KiB)",
                    bar_len / 1024
                );
                found.usb.push(crate::usb::Function {
                    name: format!("{function}"),
                    bar,
                    bar_len,
                });
            }
            Err(message) => log!("pci: xhci at {function}: {message}"),
        }
    }
    Ok(found)
}

/// A transport with its queue addresses written in halves (the network card's
/// and the GPU's).
fn split_transport(
    access: &EcamAccess,
    function: DeviceFunction,
    transport: PciTransport,
) -> Result<SplitTransport, &'static str> {
    let phys = split::common_cfg(access, function).ok_or("no common configuration found")?;
    let common = memory::map_physical(phys, split::COMMON_CFG_BYTES, DEVICE)?;
    Ok(SplitTransport::new(transport, common))
}

/// BAR0 of an NVMe or xHCI controller, with memory decoding and bus mastering
/// turned on.
fn memory_bar0(
    root: &mut PciRoot<EcamAccess>,
    function: DeviceFunction,
) -> Result<(u64, u64), &'static str> {
    let (_, command) = root.get_status_command(function);
    // Sizing a BAR rewrites it: keep decoding off meanwhile.
    root.set_command(function, command - Command::MEMORY_SPACE);
    let bar = root.bar_info(function, 0);
    root.set_command(
        function,
        command | Command::MEMORY_SPACE | Command::BUS_MASTER,
    );
    match bar {
        Ok(Some(BarInfo::Memory { address, size, .. })) if address != 0 && size != 0 => {
            Ok((address, size))
        }
        Ok(_) => Err("BAR0 is not an assigned memory BAR"),
        Err(_) => Err("BAR0 cannot be read"),
    }
}
