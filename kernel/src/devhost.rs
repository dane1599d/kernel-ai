//! The kernel's `devkit::Host` (ADR 0007 section 2), for the drivers that run
//! in ring 0: DMA from the frame allocator, below 4 GiB when the device needs
//! it, and BAR0 mapped uncached. With no IOMMU yet, a device address is the
//! physical address. Kernel drivers run from their interrupt handler or the
//! timer, so `wait` only waits out its timeout.

use crate::{hpet, log, memory};
use core::ptr::NonNull;
use devkit::{Dma, DmaError, Host, Mmio, Zone};

/// 4 GiB, the end of `Zone::Below4G`.
const FOUR_GIB: u64 = 1 << 32;

pub struct KernelHost {
    /// BAR0's physical address and length, as firmware assigned them.
    bar0: (u64, u64),
    vector: Option<u8>,
}

impl KernelHost {
    pub fn new(bar0: (u64, u64), vector: Option<u8>) -> KernelHost {
        KernelHost { bar0, vector }
    }
}

impl Host for KernelHost {
    fn dma(&mut self, pages: usize, zone: Zone) -> Result<Dma, DmaError> {
        let count = pages as u64;
        let phys = memory::with(|m| match zone {
            Zone::Any => m.allocate_contiguous(count),
            Zone::Below4G => m.allocate_contiguous_below(FOUR_GIB, count),
        })
        .ok_or(DmaError::NoMemory)?;
        let len = pages * mm::PAGE_SIZE as usize;
        let cpu = NonNull::new((memory::hhdm() + phys) as *mut u8).ok_or(DmaError::NoMemory)?;
        // SAFETY: the frames were just allocated for this driver alone, the
        // direct map covers them, and they are never freed.
        unsafe {
            core::ptr::write_bytes(cpu.as_ptr(), 0, len);
            Ok(Dma::new(cpu, phys, len))
        }
    }

    fn mmio(&mut self, bar: u8, offset: u64, len: u64) -> Result<Mmio, DmaError> {
        let (base, size) = self.bar0;
        // Bounded by the BAR firmware assigned, so neither a garbage BAR near
        // 2^64 nor a driver's offset can overflow or map past its end.
        let phys = match bar {
            0 => pci::bar_range(base, size, offset, len),
            _ => None,
        };
        let Some(phys) = phys else {
            log!(
                "devhost: BAR{bar} {offset:#x}+{len:#x} is outside BAR0 ({base:#x}, {size:#x} bytes)"
            );
            return Err(DmaError::OutOfRange);
        };
        let virt = memory::map_physical(phys, len, crate::virtio::hal::DEVICE)
            .map_err(|_| DmaError::NoMemory)?;
        let regs = NonNull::new(virt as *mut u32).ok_or(DmaError::NoMemory)?;
        // SAFETY: `len` bytes of the BAR were just mapped uncached, for good.
        Ok(unsafe { Mmio::new(regs, len) })
    }

    fn irqs(&mut self, wanted: u16) -> u16 {
        u16::from(wanted > 0 && self.vector.is_some())
    }

    fn wait(&mut self, _: u64, timeout_ns: u64) -> u64 {
        self.delay(timeout_ns);
        0
    }

    /// No kernel driver needs its configuration space after the probe yet.
    fn cfg_read(&self, _: u16) -> u32 {
        u32::MAX
    }

    fn cfg_write(&mut self, _: u16, _: u32) {}

    fn now_ns(&self) -> u64 {
        hpet::nanos()
    }
}
