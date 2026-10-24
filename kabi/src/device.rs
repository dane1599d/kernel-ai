//! Device records (ADR 0007 section 3): what [`crate::call::DEV_LIST`] fills in,
//! one 64-byte record per PCI function a driver took, for the agent's
//! `device_list` action and anyone tracking hardware.

/// Version of [`DeviceRecord`]'s layout.
pub const VERSION: u16 = 1;
/// Records the kernel keeps at most.
pub const MAX_RECORDS: usize = 64;
/// Bytes of a driver name.
pub const DRIVER_BYTES: usize = 16;

/// Where the driver runs.
pub mod place {
    pub const KERNEL: u8 = 0;
    pub const USER: u8 = 1;
}

/// What the driver is doing with the device.
pub mod state {
    pub const BOUND: u8 = 0;
    pub const RUNNING: u8 = 1;
    pub const RESETTING: u8 = 2;
    pub const RESTARTING: u8 = 3;
    pub const FAILED: u8 = 4;
    pub const OFF: u8 = 5;
}

/// How the device's interrupts reach its driver.
pub mod irq_mode {
    pub const POLLED: u8 = 0;
    pub const MSI: u8 = 1;
    pub const MSI_X: u8 = 2;
}

/// Whether an IOMMU confines the device's DMA.
pub mod iommu {
    pub const UNCONFINED: u8 = 0;
    pub const CONFINED: u8 = 1;
}

/// One bound device.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceRecord {
    pub version: u16,
    /// Its place in the kernel's list.
    pub index: u16,
    pub segment: u16,
    pub bus: u8,
    /// Device (bits 3-7) and function (bits 0-2).
    pub devfn: u8,
    pub vendor: u16,
    pub device: u16,
    /// Class, subclass and programming interface.
    pub class: [u8; 3],
    pub place: u8,
    pub state: u8,
    pub irq_mode: u8,
    pub iommu: u8,
    pub _r: u8,
    /// The driver's name, zero-padded.
    pub driver: [u8; DRIVER_BYTES],
    pub resets: u32,
    pub interrupts: u64,
    pub dma_pages: u32,
    pub last_error: u32,
    pub _pad: [u8; 8],
}

const _: () = assert!(core::mem::size_of::<DeviceRecord>() == 64);

impl DeviceRecord {
    /// The driver's name, up to its first zero byte.
    pub fn driver(&self) -> &str {
        let end = self
            .driver
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(DRIVER_BYTES);
        core::str::from_utf8(&self.driver[..end]).unwrap_or("?")
    }

    /// Sets the driver's name, cut to [`DRIVER_BYTES`].
    pub fn set_driver(&mut self, name: &str) {
        self.driver = [0; DRIVER_BYTES];
        let n = name.len().min(DRIVER_BYTES);
        self.driver[..n].copy_from_slice(&name.as_bytes()[..n]);
    }

    pub fn device_number(&self) -> u8 {
        self.devfn >> 3
    }

    pub fn function(&self) -> u8 {
        self.devfn & 7
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_keeps_its_driver_name_and_place() {
        let mut record = DeviceRecord {
            version: VERSION,
            bus: 1,
            devfn: 3 << 3 | 2,
            ..DeviceRecord::default()
        };
        record.set_driver("virtio-net");
        assert_eq!(record.driver(), "virtio-net");
        assert_eq!((record.device_number(), record.function()), (3, 2));
        record.set_driver("a-name-longer-than-sixteen");
        assert_eq!(record.driver(), "a-name-longer-th");
    }
}
