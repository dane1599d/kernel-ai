//! The driver contract (ADR 0007 section 2): one interface a driver crate is
//! written against, with a host in the kernel (`KernelHost`) today and one in
//! ring 3 (`user/devlib`'s `UserHost`) later, so moving a driver between rings
//! swaps its host, not its code. Driver crates depend on this and never on the
//! kernel. Nothing here allocates or panics on a device's say-so.

#![no_std]

use core::ptr::NonNull;

/// Where DMA memory may lie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Any,
    /// For a device that cannot address memory above 4 GiB.
    Below4G,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaError {
    /// No contiguous memory of that size in that zone.
    NoMemory,
    /// The request lies outside what the device has (a BAR, a range).
    OutOfRange,
    /// The host cannot do this.
    Unsupported,
}

/// Zeroed, physically contiguous memory a device may read and write.
#[derive(Debug)]
pub struct Dma {
    cpu: NonNull<u8>,
    device: u64,
    len: usize,
}

// SAFETY: the memory belongs to one driver, which may move between threads;
// nothing about it is tied to the thread that allocated it.
unsafe impl Send for Dma {}

impl Dma {
    /// # Safety
    /// `cpu` must point at `len` bytes the driver alone may use for as long as
    /// the `Dma` lives, which the device reaches at `device`.
    pub unsafe fn new(cpu: NonNull<u8>, device: u64, len: usize) -> Dma {
        Dma { cpu, device, len }
    }

    /// Where the CPU reaches the memory.
    pub fn cpu(&self) -> NonNull<u8> {
        self.cpu
    }

    /// The address the device uses: an IOVA under an IOMMU, the physical
    /// address without one. Never "phys" in a driver crate.
    pub fn device(&self) -> u64 {
        self.device
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// A device's register window, mapped uncached; every access is bounds-checked
/// and 32-bit aligned, so a bad offset reads all ones and writes nothing.
#[derive(Clone, Copy, Debug)]
pub struct Mmio {
    base: NonNull<u32>,
    len: u64,
}

// SAFETY: device registers are not tied to a thread; whoever holds the
// `Mmio` serialises its accesses, as a `&self` method on a `Send` value does.
unsafe impl Send for Mmio {}

impl Mmio {
    /// # Safety
    /// `base` must map `len` bytes of device registers, uncached, for as long
    /// as the `Mmio` is used.
    pub unsafe fn new(base: NonNull<u32>, len: u64) -> Mmio {
        Mmio { base, len }
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn at(&self, offset: u64) -> Option<*mut u32> {
        let inside = offset.checked_add(4).is_some_and(|end| end <= self.len);
        (inside && offset.is_multiple_of(4)).then(|| {
            // In bounds and aligned, checked above.
            self.base.as_ptr().wrapping_add((offset / 4) as usize)
        })
    }

    /// The register at `offset`, or all ones outside the window.
    pub fn read32(&self, offset: u64) -> u32 {
        match self.at(offset) {
            // SAFETY: inside the mapped window (see `new`).
            Some(register) => unsafe { register.read_volatile() },
            None => u32::MAX,
        }
    }

    /// Writes the register at `offset`; false (and nothing written) outside.
    pub fn write32(&self, offset: u64, value: u32) -> bool {
        match self.at(offset) {
            Some(register) => {
                // SAFETY: as in read32.
                unsafe { register.write_volatile(value) };
                true
            }
            None => false,
        }
    }
}

/// What a host gives the driver of one device.
pub trait Host {
    /// `pages` zeroed, contiguous 4 KiB pages in `zone`. Never panics.
    fn dma(&mut self, pages: usize, zone: Zone) -> Result<Dma, DmaError>;
    /// `len` bytes of BAR `bar` from `offset`, uncached and bounds-checked.
    fn mmio(&mut self, bar: u8, offset: u64, len: u64) -> Result<Mmio, DmaError>;
    /// Up to `wanted` interrupt vectors: MSI-X, else MSI, else 0 (polled).
    fn irqs(&mut self, wanted: u16) -> u16;
    /// Waits for any of the vectors in `mask` (bit n is vector n) or
    /// `timeout_ns`; returns the bits that fired, unmasking those waited on.
    fn wait(&mut self, mask: u64, timeout_ns: u64) -> u64;
    /// The device's own configuration space.
    fn cfg_read(&self, offset: u16) -> u32;
    /// Writes to its own configuration space; the host may refuse some.
    fn cfg_write(&mut self, offset: u16, value: u32);
    /// A monotonic clock.
    fn now_ns(&self) -> u64;

    /// Waits about `ns` nanoseconds without sleeping.
    fn delay(&self, ns: u64) {
        let start = self.now_ns();
        while self.now_ns().wrapping_sub(start) < ns {
            core::hint::spin_loop();
        }
    }
}

/// What one `poll` did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub completions: u32,
    /// Work is left past the budget: poll again before unmasking.
    pub more: bool,
}

/// What a driver reads from its device's status registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Ok,
    Stalled,
    Fatal(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverError {
    Timeout,
    Fatal(u32),
}

/// A driver, as its host drives it.
pub trait Driver {
    /// Handles at most `budget` completions (64 for network and block).
    fn poll(&mut self, budget: u32) -> Progress;
    /// Puts the device back to a known state and starts it again.
    fn reset(&mut self) -> Result<(), DriverError>;
    fn health(&self) -> Health;
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;

    #[test]
    fn mmio_is_bounds_checked_and_aligned() {
        let mut registers = vec![0u32; 4];
        let base = NonNull::new(registers.as_mut_ptr()).unwrap();
        // SAFETY: `registers` outlives `mmio` and holds 16 bytes.
        let mmio = unsafe { Mmio::new(base, 16) };
        assert!(mmio.write32(12, 0xABCD));
        assert_eq!(mmio.read32(12), 0xABCD);
        assert!(!mmio.write32(16, 1), "past the end");
        assert!(!mmio.write32(2, 1), "unaligned");
        assert!(!mmio.write32(u64::MAX - 1, 1), "an offset that overflows");
        assert_eq!(mmio.read32(16), u32::MAX);
        assert_eq!(registers, [0, 0, 0, 0xABCD]);
    }

    #[test]
    fn dma_says_where_the_device_and_the_cpu_reach_it() {
        let mut page = vec![0u8; 4096];
        let cpu = NonNull::new(page.as_mut_ptr()).unwrap();
        // SAFETY: `page` outlives `dma` and holds 4096 bytes.
        let dma = unsafe { Dma::new(cpu, 0x1234_5000, 4096) };
        assert_eq!(
            (dma.cpu(), dma.device(), dma.len()),
            (cpu, 0x1234_5000, 4096)
        );
        assert!(!dma.is_empty());
    }
}
