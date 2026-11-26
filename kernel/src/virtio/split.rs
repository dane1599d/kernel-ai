//! A virtio PCI transport that writes the 64-bit queue addresses as two 32-bit
//! halves, as the VirtIO 1.2 spec requires of a driver (4.1.3.1). virtio-drivers
//! 0.13 writes each with one 64-bit store, which QEMU accepts and VirtualBox's
//! virtio-net never completes: the vCPU retries the store forever. Everything but
//! `queue_set` goes to the wrapped transport unchanged. It also gives one
//! queue an MSI-X entry ([`SplitTransport::route_queue`]): the network card's
//! receive queue, so frames raise an interrupt (P5 plan 3.2).

use super::pci::EcamAccess;
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::pci::bus::{ConfigurationAccess, DeviceFunction};
use virtio_drivers::transport::{DeviceStatus, DeviceType, InterruptStatus, Transport};
use virtio_drivers::{PhysAddr, Result};
use zerocopy::{FromBytes, Immutable, IntoBytes};

/// Vendor-specific PCI capability, which virtio's structures are.
const CAP_VENDOR: u8 = 0x09;
/// The common configuration structure's `cfg_type`.
const COMMON_CFG: u8 = 1;
/// Offsets in the common configuration structure.
const MSIX_CONFIG: u64 = 16;
const QUEUE_SELECT: u64 = 22;
const QUEUE_SIZE: u64 = 24;
const QUEUE_MSIX_VECTOR: u64 = 26;
const QUEUE_ENABLE: u64 = 28;
const QUEUE_DESC: u64 = 32;
const QUEUE_DRIVER: u64 = 40;
const QUEUE_DEVICE: u64 = 48;
/// Bytes of it the driver writes.
pub const COMMON_CFG_BYTES: u64 = 56;
/// No MSI-X entry: the event is not signalled (VirtIO 1.2, 4.1.4.3).
const NO_VECTOR: u16 = 0xFFFF;

/// VIRTIO_F_ACCESS_PLATFORM: the device's DMA goes through the platform's
/// IOMMU (VirtIO 1.2, 6).
const ACCESS_PLATFORM: u64 = 1 << 33;
static ACCESS_PLATFORM_TAKEN: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Whether a device behind this transport negotiated ACCESS_PLATFORM.
pub fn access_platform_taken() -> bool {
    ACCESS_PLATFORM_TAKEN.load(core::sync::atomic::Ordering::Relaxed)
}

pub struct SplitTransport {
    inner: PciTransport,
    /// The common configuration structure, mapped uncached.
    common: u64,
    /// The queue given an MSI-X entry, and the entry.
    routed: Option<(u16, u16)>,
    /// Whether the device took that entry when the queue was set up.
    routed_taken: bool,
}

/// Where `function`'s common configuration structure lies, from its
/// capabilities: the BAR it is in and its offset there, both as the device
/// says. The caller bounds them against that BAR before mapping anything.
pub fn common_cfg(ecam: &EcamAccess, function: DeviceFunction) -> Option<(u8, u64)> {
    let mut at = (ecam.read_word(function, 0x34) & 0xFC) as u8;
    for _ in 0..48 {
        if at == 0 {
            return None;
        }
        let header = ecam.read_word(function, at);
        let next = (header >> 8) as u8;
        if header as u8 == CAP_VENDOR && (header >> 24) as u8 == COMMON_CFG {
            // A capability placed so its fields run past the 256-byte header
            // is garbage, not an overflow.
            let bar = (ecam.read_word(function, at.checked_add(4)?) & 0xFF) as u8;
            let offset = u64::from(ecam.read_word(function, at.checked_add(8)?));
            return Some((bar, offset));
        }
        at = next & 0xFC;
    }
    None
}

impl SplitTransport {
    /// Wraps `inner`, whose common configuration is mapped at `common`.
    pub fn new(inner: PciTransport, common: u64) -> Self {
        Self {
            inner,
            common,
            routed: None,
            routed_taken: false,
        }
    }

    /// Gives `queue` MSI-X table entry `entry` when it is set up; every other
    /// queue and the configuration change get none. MSI-X must be on.
    pub fn route_queue(&mut self, queue: u16, entry: u16) {
        self.routed = Some((queue, entry));
    }

    /// Whether the routed queue was set up with its entry: the device reads
    /// back NO_VECTOR when it could not take it.
    pub fn routed(&self) -> bool {
        self.routed_taken
    }

    fn write16(&mut self, offset: u64, value: u16) {
        // SAFETY: `common` maps the device's common configuration structure for at
        // least COMMON_CFG_BYTES, uncached, and every offset used is inside it.
        unsafe { ((self.common + offset) as *mut u16).write_volatile(value) }
    }

    fn read16(&self, offset: u64) -> u16 {
        // SAFETY: as in write16.
        unsafe { ((self.common + offset) as *const u16).read_volatile() }
    }

    fn write64(&mut self, offset: u64, value: u64) {
        // SAFETY: as in write16; the two halves are 4-aligned like the field.
        unsafe {
            ((self.common + offset) as *mut u32).write_volatile(value as u32);
            ((self.common + offset + 4) as *mut u32).write_volatile((value >> 32) as u32);
        }
    }
}

impl Transport for SplitTransport {
    fn device_type(&self) -> DeviceType {
        self.inner.device_type()
    }

    fn read_device_features(&mut self) -> u64 {
        self.inner.read_device_features()
    }

    fn write_driver_features(&mut self, driver_features: u64) {
        // S07-2 (spike only): virtio-drivers 0.13 accepts no ACCESS_PLATFORM,
        // and QEMU refuses FEATURES_OK from a driver that leaves out one it
        // offers, so the transport takes it whenever the device offers it.
        let offered = self.inner.read_device_features() & ACCESS_PLATFORM != 0;
        if offered {
            ACCESS_PLATFORM_TAKEN.store(true, core::sync::atomic::Ordering::Relaxed);
        }
        let features = match offered {
            true => driver_features | ACCESS_PLATFORM,
            false => driver_features,
        };
        self.inner.write_driver_features(features)
    }

    fn max_queue_size(&mut self, queue: u16) -> u32 {
        self.inner.max_queue_size(queue)
    }

    fn notify(&mut self, queue: u16) {
        self.inner.notify(queue)
    }

    fn get_status(&self) -> DeviceStatus {
        self.inner.get_status()
    }

    fn set_status(&mut self, status: DeviceStatus) {
        self.inner.set_status(status)
    }

    fn set_guest_page_size(&mut self, guest_page_size: u32) {
        self.inner.set_guest_page_size(guest_page_size)
    }

    fn requires_legacy_layout(&self) -> bool {
        self.inner.requires_legacy_layout()
    }

    fn queue_set(
        &mut self,
        queue: u16,
        size: u32,
        descriptors: PhysAddr,
        driver_area: PhysAddr,
        device_area: PhysAddr,
    ) {
        self.write16(QUEUE_SELECT, queue);
        self.write16(QUEUE_SIZE, size as u16);
        if let Some((routed, entry)) = self.routed {
            self.write16(MSIX_CONFIG, NO_VECTOR);
            let vector = if routed == queue { entry } else { NO_VECTOR };
            self.write16(QUEUE_MSIX_VECTOR, vector);
            if routed == queue {
                self.routed_taken = self.read16(QUEUE_MSIX_VECTOR) == entry;
            }
        }
        self.write64(QUEUE_DESC, descriptors);
        self.write64(QUEUE_DRIVER, driver_area);
        self.write64(QUEUE_DEVICE, device_area);
        self.write16(QUEUE_ENABLE, 1);
    }

    fn queue_unset(&mut self, queue: u16) {
        self.inner.queue_unset(queue)
    }

    fn queue_used(&mut self, queue: u16) -> bool {
        self.inner.queue_used(queue)
    }

    fn ack_interrupt(&mut self) -> InterruptStatus {
        self.inner.ack_interrupt()
    }

    fn read_config_generation(&self) -> u32 {
        self.inner.read_config_generation()
    }

    fn read_config_space<T: FromBytes + IntoBytes>(&self, offset: usize) -> Result<T> {
        self.inner.read_config_space(offset)
    }

    fn write_config_space<T: IntoBytes + Immutable>(
        &mut self,
        offset: usize,
        value: T,
    ) -> Result<()> {
        self.inner.write_config_space(offset, value)
    }
}
