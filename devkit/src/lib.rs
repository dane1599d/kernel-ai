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
/// and naturally aligned (a 32-bit register on 4 bytes, a 16-bit one on 2), so
/// a bad offset reads all ones and writes nothing. The 8- and 16-bit accesses
/// are for devices whose registers sit at byte offsets and decode only at
/// their own width (HD Audio's CORB and RIRB controls, a stream's format).
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
        self.at_width(offset, 4).map(|p| p.cast::<u32>())
    }

    /// The byte pointer for a `width`-byte access at `offset`, if it is inside
    /// the window and aligned to `width`.
    fn at_width(&self, offset: u64, width: u64) -> Option<*mut u8> {
        let inside = offset.checked_add(width).is_some_and(|end| end <= self.len);
        (inside && offset.is_multiple_of(width)).then(|| {
            // In bounds and aligned, checked above.
            self.base
                .as_ptr()
                .cast::<u8>()
                .wrapping_add(offset as usize)
        })
    }

    /// The 8-bit register at `offset`, or all ones outside the window.
    pub fn read8(&self, offset: u64) -> u8 {
        match self.at_width(offset, 1) {
            // SAFETY: inside the mapped window (see `new`).
            Some(register) => unsafe { register.read_volatile() },
            None => u8::MAX,
        }
    }

    /// The 16-bit register at `offset` (2-aligned), or all ones outside.
    pub fn read16(&self, offset: u64) -> u16 {
        match self.at_width(offset, 2) {
            // SAFETY: inside the mapped window and 2-aligned.
            Some(register) => unsafe { register.cast::<u16>().read_volatile() },
            None => u16::MAX,
        }
    }

    /// Writes the 8-bit register at `offset`; false (and nothing written)
    /// outside.
    pub fn write8(&self, offset: u64, value: u8) -> bool {
        match self.at_width(offset, 1) {
            Some(register) => {
                // SAFETY: as in read8.
                unsafe { register.write_volatile(value) };
                true
            }
            None => false,
        }
    }

    /// Writes the 16-bit register at `offset` (2-aligned); false (and nothing
    /// written) outside or unaligned.
    pub fn write16(&self, offset: u64, value: u16) -> bool {
        match self.at_width(offset, 2) {
            Some(register) => {
                // SAFETY: as in read16.
                unsafe { register.cast::<u16>().write_volatile(value) };
                true
            }
            None => false,
        }
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
    fn byte_and_word_accesses_are_bounds_checked_and_naturally_aligned() {
        let mut registers = vec![0u32; 2];
        let base = NonNull::new(registers.as_mut_ptr()).unwrap();
        // SAFETY: `registers` outlives `mmio` and holds 8 bytes.
        let mmio = unsafe { Mmio::new(base, 8) };
        assert!(mmio.write8(5, 0x42));
        assert!(mmio.write16(6, 0xBEEF));
        assert!(mmio.write16(0, 0x1234));
        assert_eq!(mmio.read8(5), 0x42);
        assert_eq!(mmio.read16(6), 0xBEEF);
        assert_eq!(mmio.read32(4), 0xBEEF_4200, "little-endian bytes");
        assert!(!mmio.write16(3, 1), "unaligned");
        assert!(!mmio.write16(7, 1), "unaligned and past the end");
        assert!(!mmio.write8(8, 1), "past the end");
        assert!(!mmio.write16(u64::MAX, 1), "an offset that overflows");
        assert_eq!(mmio.read16(8), u16::MAX);
        assert_eq!(mmio.read8(8), u8::MAX);
        assert_eq!(mmio.read16(1), u16::MAX, "unaligned reads all ones");
        assert_eq!(registers, [0x1234, 0xBEEF_4200]);
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
