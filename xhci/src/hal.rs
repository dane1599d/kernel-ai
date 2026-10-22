//! What the driver needs from its host: DMA memory, the registers, and a clock.
//! The kernel implements it today; a user-space driver process would implement
//! it over mapped device memory and a pinned buffer pool.

use core::ptr::NonNull;

/// Memory the controller reads and writes: physically contiguous, zeroed when
/// handed out, and never moved or freed while the controller may touch it.
/// Dropping a `Dma` does not free its memory.
pub struct Dma {
    phys: u64,
    virt: NonNull<u8>,
    len: usize,
}

impl Dma {
    /// # Safety
    ///
    /// `virt` must point at `len` bytes of memory that is the CPU's view of the
    /// physically contiguous range at `phys`, aligned to 64 bytes, used by
    /// nothing else, and valid for as long as the `Dma` and the controller live.
    pub unsafe fn new(phys: u64, virt: NonNull<u8>, len: usize) -> Self {
        Self { phys, virt, len }
    }

    pub fn phys(&self) -> u64 {
        self.phys
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Panics unless `bytes` from `offset` lie inside the memory and, for a
    /// dword access, `offset` is 4-aligned.
    fn check(&self, offset: usize, bytes: usize, dword: bool) {
        assert!(
            (!dword || offset.is_multiple_of(4)) && offset + bytes <= self.len,
            "xhci: an access outside device memory"
        );
    }

    pub fn read32(&self, offset: usize) -> u32 {
        self.check(offset, 4, true);
        // SAFETY: checked to be inside the memory and 4-aligned; the device may
        // write it at any time, so the read is volatile.
        unsafe { self.virt.as_ptr().add(offset).cast::<u32>().read_volatile() }
    }

    pub fn write32(&mut self, offset: usize, value: u32) {
        self.check(offset, 4, true);
        // SAFETY: as in read32.
        unsafe {
            self.virt
                .as_ptr()
                .add(offset)
                .cast::<u32>()
                .write_volatile(value)
        }
    }

    pub fn write64(&mut self, offset: usize, value: u64) {
        self.write32(offset, value as u32);
        self.write32(offset + 4, (value >> 32) as u32);
    }

    /// Writes `dwords` from `offset` on.
    pub fn write_dwords(&mut self, offset: usize, dwords: &[u32]) {
        for (index, &dword) in dwords.iter().enumerate() {
            self.write32(offset + 4 * index, dword);
        }
    }

    /// Copies `out.len()` bytes from `offset` into `out`.
    pub fn read_bytes(&self, offset: usize, out: &mut [u8]) {
        self.check(offset, out.len(), false);
        for (index, byte) in out.iter_mut().enumerate() {
            // SAFETY: checked to lie inside the memory.
            *byte = unsafe { self.virt.as_ptr().add(offset + index).read_volatile() };
        }
    }

    /// Zeroes `len` bytes from `offset`.
    pub fn zero(&mut self, offset: usize, len: usize) {
        self.check(offset, len, false);
        for index in 0..len {
            // SAFETY: checked to lie inside the memory.
            unsafe { self.virt.as_ptr().add(offset + index).write_volatile(0) };
        }
    }
}

// SAFETY: the memory belongs to the Dma alone; which CPU touches it does not
// matter, and `&mut self` on every write keeps it to one writer at a time.
unsafe impl Send for Dma {}

/// The host's side of the driver.
pub trait Hal {
    /// `pages` zeroed, physically contiguous 4 KiB pages. When `below_4g`, the
    /// memory must lie below 4 GiB (a controller without 64-bit addressing).
    fn dma(&mut self, pages: usize, below_4g: bool) -> Option<Dma>;

    /// Reads the 32-bit register at `offset` in BAR0.
    fn read32(&self, offset: usize) -> u32;

    /// Writes the 32-bit register at `offset` in BAR0.
    fn write32(&self, offset: usize, value: u32);

    /// A monotonic clock in nanoseconds, for timeouts.
    fn now_ns(&self) -> u64;

    /// Whether the host takes the controller's interrupter 0 interrupt. If so the
    /// driver enables it and [`crate::Controller::poll`] acknowledges it;
    /// otherwise the host calls `poll` on a timer.
    fn interrupts(&self) -> bool {
        false
    }

    /// Waits about `ns` nanoseconds (port power-up and reset recovery).
    fn delay(&self, ns: u64) {
        let start = self.now_ns();
        while self.now_ns().wrapping_sub(start) < ns {
            core::hint::spin_loop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    #[test]
    fn dma_reads_and_writes_inside_its_memory() {
        let mut backing = vec![0u64; 8];
        let virt = NonNull::new(backing.as_mut_ptr().cast::<u8>()).unwrap();
        // SAFETY: the vector outlives the Dma and nothing else touches it.
        let mut dma = unsafe { Dma::new(0x1000, virt, 64) };
        dma.write64(8, 0x1122_3344_5566_7788);
        assert_eq!(dma.read32(8), 0x5566_7788);
        assert_eq!(dma.read32(12), 0x1122_3344);
        dma.write_dwords(16, &[1, 2]);
        let mut bytes = [0u8; 8];
        dma.read_bytes(16, &mut bytes);
        assert_eq!(bytes, [1, 0, 0, 0, 2, 0, 0, 0]);
        dma.zero(16, 8);
        assert_eq!(dma.read32(16), 0);
        // Byte reads need no alignment.
        let mut three = [0u8; 3];
        dma.read_bytes(9, &mut three);
        assert_eq!(three, [0x77, 0x66, 0x55]);
        assert_eq!((dma.phys(), dma.len(), dma.is_empty()), (0x1000, 64, false));
    }

    #[test]
    #[should_panic(expected = "outside device memory")]
    fn an_access_past_the_end_panics() {
        let mut backing = vec![0u64; 2];
        let virt = NonNull::new(backing.as_mut_ptr().cast::<u8>()).unwrap();
        // SAFETY: as above.
        let dma = unsafe { Dma::new(0, virt, 16) };
        dma.read32(16);
    }
}
