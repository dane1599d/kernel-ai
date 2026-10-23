//! PCIe configuration access through the ECAM windows from the MCFG table, the
//! walk over every bus behind each window (the `pci` crate), and the binding of
//! each function to its driver from `devmatch`'s table: the virtio network card,
//! block devices and GPU, the NVMe controllers and the xHCI (USB 3) controllers.

use super::hal::{DEVICE, KernelHal};
use super::split::{self, SplitTransport};
use crate::{log, memory};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::vec::Vec;
use devmatch::{Ids, Override};
use spin::Mutex;
use virtio_drivers::transport::DeviceType;
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::pci::bus::{
    BarInfo, Command, ConfigurationAccess, DeviceFunction, PciRoot,
};

/// The virtual address of each (segment, bus) mapped so far: one bus is 1 MiB
/// of ECAM, mapped the first time the walk or a driver reaches it.
static MAPPED: Mutex<BTreeMap<(u16, u8), u64>> = Mutex::new(BTreeMap::new());

/// Configuration space for one MCFG window.
#[derive(Clone, Copy)]
pub struct EcamAccess {
    /// Where bus 0 would be, even if the window starts at a later bus.
    base: u64,
    segment: u16,
    start_bus: u8,
    end_bus: u8,
}

impl EcamAccess {
    fn new(window: &acpitables::Ecam) -> EcamAccess {
        EcamAccess {
            base: window.base,
            segment: window.segment,
            start_bus: window.start_bus,
            end_bus: window.end_bus,
        }
    }

    /// Where `offset` of a function's configuration space is mapped, mapping its
    /// bus first if need be.
    fn address(&self, bus: u8, device: u8, function: u8, offset: u16) -> Option<u64> {
        let in_window = (self.start_bus..=self.end_bus).contains(&bus);
        if !in_window || device >= 32 || function >= 8 || offset >= 4096 {
            return None;
        }
        let mut mapped = MAPPED.lock();
        let bus_base = match mapped.get(&(self.segment, bus)) {
            Some(&base) => base,
            None => {
                let phys = self.base + (u64::from(bus) << 20);
                let base = memory::map_physical(phys, 1 << 20, DEVICE).ok()?;
                mapped.insert((self.segment, bus), base);
                base
            }
        };
        Some(
            bus_base
                + ((u64::from(device) << 15)
                    | (u64::from(function) << 12)
                    | u64::from(offset & 0xFFC)),
        )
    }

    fn load(address: Option<u64>) -> u32 {
        match address {
            // SAFETY: the address lies in an uncached ECAM bus mapped by `address`.
            Some(address) => unsafe { (address as *const u32).read_volatile() },
            None => u32::MAX,
        }
    }

    fn store(address: Option<u64>, data: u32) {
        if let Some(address) = address {
            // SAFETY: as in load.
            unsafe { (address as *mut u32).write_volatile(data) }
        }
    }
}

impl ConfigurationAccess for EcamAccess {
    fn read_word(&self, f: DeviceFunction, offset: u8) -> u32 {
        Self::load(self.address(f.bus, f.device, f.function, offset.into()))
    }

    fn write_word(&mut self, f: DeviceFunction, offset: u8, data: u32) {
        Self::store(
            self.address(f.bus, f.device, f.function, offset.into()),
            data,
        );
    }

    unsafe fn unsafe_clone(&self) -> Self {
        *self
    }
}

impl pci::ConfigRegionAccess for EcamAccess {
    unsafe fn read(&self, a: pci::PciAddress, offset: u16) -> u32 {
        Self::load(self.address(a.bus(), a.device(), a.function(), offset))
    }

    unsafe fn write(&self, a: pci::PciAddress, offset: u16, value: u32) {
        Self::store(
            self.address(a.bus(), a.device(), a.function(), offset),
            value,
        );
    }
}

/// The devices the drivers take, each with its transport or BAR set up.
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

/// Walks every bus of every MCFG window once and sets up each function that
/// `devmatch` gives a driver, unless `drv=` on `cmdline` turns that driver off.
pub fn probe(platform: &acpitables::Platform, cmdline: &str) -> Result<Found, &'static str> {
    if platform.ecam.is_empty() {
        return Err("pci: no PCIe configuration window (MCFG)");
    }
    let drv = devmatch::drv_value(cmdline);
    let mut found = Found {
        net: None,
        net_failed: false,
        disks: Vec::new(),
        gpu: None,
        nvme: Vec::new(),
        usb: Vec::new(),
    };
    for window in &platform.ecam {
        let access = EcamAccess::new(window);
        let functions = pci::walk(&access, window.segment, window.start_bus, window.end_bus);
        log!(
            "pci: segment {} buses {:02x}-{:02x}: {} functions",
            window.segment,
            window.start_bus,
            window.end_bus,
            functions.len()
        );
        let mut root = PciRoot::new(access);
        for f in functions {
            let ids = Ids {
                vendor: f.vendor,
                device: f.device,
                class: f.class,
            };
            let Some(row) = devmatch::lookup(devmatch::TABLE, ids) else {
                continue;
            };
            let at = DeviceFunction {
                bus: f.address.bus(),
                device: f.address.device(),
                function: f.address.function(),
            };
            match devmatch::override_for(drv, row.driver) {
                Some(Override::Off) => {
                    log!("pci: {} at {at} is off (drv=)", row.driver);
                    continue;
                }
                Some(Override::User) => {
                    log!(
                        "pci: {} has no ring-3 host yet; it stays in the kernel",
                        row.driver
                    );
                }
                Some(Override::Kernel) | None => {}
            }
            if window.segment != 0 {
                log!(
                    "pci: {} at segment {} {at}: only segment 0 is bound yet",
                    row.driver,
                    window.segment
                );
                continue;
            }
            bind(&mut found, &mut root, &access, row.driver, at, ids);
        }
    }
    Ok(found)
}

/// Sets up the function at `at` for `driver`.
fn bind(
    found: &mut Found,
    root: &mut PciRoot<EcamAccess>,
    access: &EcamAccess,
    driver: &'static str,
    at: DeviceFunction,
    ids: Ids,
) {
    if matches!(driver, "nvme" | "xhci") {
        match memory_bar0(root, at) {
            Ok((bar, bar_len)) => {
                log!(
                    "pci: {driver} at {at}, BAR0 {bar:#x} ({} KiB)",
                    bar_len / 1024
                );
                let name = format!("{at}");
                match driver {
                    "nvme" => found
                        .nvme
                        .push(crate::nvme::Function { name, bar, bar_len }),
                    _ => found.usb.push(crate::usb::Function { name, bar, bar_len }),
                }
            }
            Err(message) => log!("pci: {driver} at {at}: {message}"),
        }
        return;
    }
    let kind = match driver {
        "virtio-net" => DeviceType::Network,
        "virtio-gpu" => DeviceType::GPU,
        "virtio-blk" => DeviceType::Block,
        _ => {
            log!("pci: no kernel driver called {driver} for {at}");
            return;
        }
    };
    if kind == DeviceType::Network && found.net.is_some() {
        log!("pci: ignoring a second virtio network card at {at}");
        return;
    }
    if kind == DeviceType::GPU && found.gpu.is_some() {
        log!("pci: ignoring a second virtio-gpu at {at}");
        return;
    }
    log!("pci: {driver} at {at} (id 1af4:{:04x})", ids.device);
    // Firmware assigned the BARs; make sure the device decodes them and may DMA.
    let (_, command) = root.get_status_command(at);
    root.set_command(at, command | Command::MEMORY_SPACE | Command::BUS_MASTER);
    let Ok(transport) = PciTransport::new::<KernelHal, _>(root, at) else {
        log!("pci: {driver} at {at}: transport setup failed");
        found.net_failed |= kind == DeviceType::Network;
        return;
    };
    match kind {
        DeviceType::Network => match split_transport(access, at, transport) {
            Ok(transport) => found.net = Some(transport),
            Err(message) => {
                log!("pci: {driver} at {at}: {message}");
                found.net_failed = true;
            }
        },
        DeviceType::GPU => match split_transport(access, at, transport) {
            Ok(transport) => found.gpu = Some(transport),
            Err(message) => log!("pci: {driver} at {at}: {message}"),
        },
        _ => found.disks.push(transport),
    }
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
