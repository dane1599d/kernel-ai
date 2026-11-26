//! A virtio PCI transport over the card's BAR as `netd` maps it (VirtIO 1.2,
//! 4.1.4): the common configuration, notification, ISR and device structures
//! at the offsets the kernel found. One MSI-X entry, 0, goes to the receive
//! queue, as the kernel's `SplitTransport::route_queue` gives it; the queue
//! addresses are written in 32-bit halves, as the spec asks.

use virtio_drivers::transport::{DeviceStatus, DeviceType, InterruptStatus, Transport};
use virtio_drivers::{Error, PhysAddr, Result};
use zerocopy::{FromBytes, Immutable, IntoBytes};

const DEVICE_FEATURE_SELECT: u64 = 0;
const DEVICE_FEATURE: u64 = 4;
const DRIVER_FEATURE_SELECT: u64 = 8;
const DRIVER_FEATURE: u64 = 12;
const MSIX_CONFIG: u64 = 16;
const DEVICE_STATUS: u64 = 20;
const CONFIG_GENERATION: u64 = 21;
const QUEUE_SELECT: u64 = 22;
const QUEUE_SIZE: u64 = 24;
const QUEUE_MSIX_VECTOR: u64 = 26;
const QUEUE_ENABLE: u64 = 28;
const QUEUE_NOTIFY_OFF: u64 = 30;
const QUEUE_DESC: u64 = 32;
const QUEUE_DRIVER: u64 = 40;
const QUEUE_DEVICE: u64 = 48;
const NO_VECTOR: u16 = 0xFFFF;
/// The queue given MSI-X entry 0: receive.
const ROUTED: u16 = 0;
const QUEUES: usize = 2;

pub struct BarTransport {
    common: u64,
    notify: u64,
    multiplier: u64,
    isr: u64,
    device: u64,
    /// Each queue's notification address, once it is set up.
    notify_at: [u64; QUEUES],
    /// Whether the receive queue took its MSI-X entry.
    pub routed: bool,
}

impl BarTransport {
    /// `bar` is where the BAR is mapped; the rest are offsets in it.
    pub fn new(bar: u64, common: u64, notify: u64, multiplier: u32, isr: u64, device: u64) -> Self {
        Self {
            common: bar + common,
            notify: bar + notify,
            multiplier: u64::from(multiplier),
            isr: bar + isr,
            device: bar + device,
            notify_at: [0; QUEUES],
            routed: false,
        }
    }

    fn r8(&self, at: u64) -> u8 {
        // SAFETY: inside the mapped BAR, uncached.
        unsafe { ((self.common + at) as *const u8).read_volatile() }
    }
    fn w8(&mut self, at: u64, v: u8) {
        // SAFETY: as in r8.
        unsafe { ((self.common + at) as *mut u8).write_volatile(v) }
    }
    fn r16(&self, at: u64) -> u16 {
        // SAFETY: as in r8.
        unsafe { ((self.common + at) as *const u16).read_volatile() }
    }
    fn w16(&mut self, at: u64, v: u16) {
        // SAFETY: as in r8.
        unsafe { ((self.common + at) as *mut u16).write_volatile(v) }
    }
    fn r32(&self, at: u64) -> u32 {
        // SAFETY: as in r8.
        unsafe { ((self.common + at) as *const u32).read_volatile() }
    }
    fn w32(&mut self, at: u64, v: u32) {
        // SAFETY: as in r8.
        unsafe { ((self.common + at) as *mut u32).write_volatile(v) }
    }
    fn w64(&mut self, at: u64, v: u64) {
        self.w32(at, v as u32);
        self.w32(at + 4, (v >> 32) as u32);
    }
}

impl Transport for BarTransport {
    fn device_type(&self) -> DeviceType {
        DeviceType::Network
    }

    fn read_device_features(&mut self) -> u64 {
        self.w32(DEVICE_FEATURE_SELECT, 0);
        let low = self.r32(DEVICE_FEATURE);
        self.w32(DEVICE_FEATURE_SELECT, 1);
        let high = self.r32(DEVICE_FEATURE);
        u64::from(high) << 32 | u64::from(low)
    }

    fn write_driver_features(&mut self, features: u64) {
        self.w32(DRIVER_FEATURE_SELECT, 0);
        self.w32(DRIVER_FEATURE, features as u32);
        self.w32(DRIVER_FEATURE_SELECT, 1);
        self.w32(DRIVER_FEATURE, (features >> 32) as u32);
    }

    fn max_queue_size(&mut self, queue: u16) -> u32 {
        self.w16(QUEUE_SELECT, queue);
        u32::from(self.r16(QUEUE_SIZE))
    }

    fn notify(&mut self, queue: u16) {
        let at = self.notify_at[usize::from(queue)];
        // SAFETY: the queue's notification address, inside the BAR.
        unsafe { (at as *mut u16).write_volatile(queue) }
    }

    fn get_status(&self) -> DeviceStatus {
        DeviceStatus::from_bits_truncate(u32::from(self.r8(DEVICE_STATUS)))
    }

    fn set_status(&mut self, status: DeviceStatus) {
        self.w8(DEVICE_STATUS, status.bits() as u8);
    }

    fn set_guest_page_size(&mut self, _: u32) {}

    fn requires_legacy_layout(&self) -> bool {
        false
    }

    fn queue_set(
        &mut self,
        queue: u16,
        size: u32,
        descriptors: PhysAddr,
        driver_area: PhysAddr,
        device_area: PhysAddr,
    ) {
        self.w16(QUEUE_SELECT, queue);
        self.w16(QUEUE_SIZE, size as u16);
        self.w16(MSIX_CONFIG, NO_VECTOR);
        let vector = if queue == ROUTED { 0 } else { NO_VECTOR };
        self.w16(QUEUE_MSIX_VECTOR, vector);
        if queue == ROUTED {
            self.routed = self.r16(QUEUE_MSIX_VECTOR) == 0;
        }
        let off = u64::from(self.r16(QUEUE_NOTIFY_OFF));
        self.notify_at[usize::from(queue)] = self.notify + off * self.multiplier;
        self.w64(QUEUE_DESC, descriptors);
        self.w64(QUEUE_DRIVER, driver_area);
        self.w64(QUEUE_DEVICE, device_area);
        self.w16(QUEUE_ENABLE, 1);
    }

    fn queue_unset(&mut self, queue: u16) {
        self.w16(QUEUE_SELECT, queue);
        self.w16(QUEUE_ENABLE, 0);
    }

    fn queue_used(&mut self, queue: u16) -> bool {
        self.w16(QUEUE_SELECT, queue);
        self.r16(QUEUE_ENABLE) == 1
    }

    fn ack_interrupt(&mut self) -> InterruptStatus {
        // SAFETY: the ISR byte, inside the BAR; reading it acknowledges.
        let isr = unsafe { (self.isr as *const u8).read_volatile() };
        InterruptStatus::from_bits_truncate(u32::from(isr))
    }

    fn read_config_generation(&self) -> u32 {
        u32::from(self.r8(CONFIG_GENERATION))
    }

    fn read_config_space<T: FromBytes + IntoBytes>(&self, offset: usize) -> Result<T> {
        let mut value = T::new_zeroed();
        for (i, byte) in value.as_mut_bytes().iter_mut().enumerate() {
            // SAFETY: the device structure, inside the BAR; byte reads have no
            // side effects there.
            *byte = unsafe { ((self.device + (offset + i) as u64) as *const u8).read_volatile() };
        }
        Ok(value)
    }

    fn write_config_space<T: IntoBytes + Immutable>(&mut self, _: usize, _: T) -> Result<()> {
        Err(Error::Unsupported)
    }
}
