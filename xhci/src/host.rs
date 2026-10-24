//! [`crate::Hal`] over [`devkit::Host`] (ADR 0007 section 2): the one adapter
//! the controller needs, so the same driver runs on the kernel's host today and
//! on `usbd`'s ring-3 host later.

use crate::{Dma, Hal};
use devkit::{Host, Mmio, Zone};

pub struct HostHal<H: Host> {
    host: H,
    /// BAR0's registers.
    regs: Mmio,
    interrupts: bool,
}

impl<H: Host> HostHal<H> {
    /// The controller's host, its registers (BAR0, as `host.mmio` mapped them)
    /// and whether interrupter 0 interrupts (else the host polls on a timer).
    pub fn new(host: H, regs: Mmio, interrupts: bool) -> Self {
        HostHal {
            host,
            regs,
            interrupts,
        }
    }

    /// Bytes of BAR0 mapped.
    pub fn mapped(&self) -> usize {
        self.regs.len() as usize
    }
}

impl<H: Host> Hal for HostHal<H> {
    fn dma(&mut self, pages: usize, below_4g: bool) -> Option<Dma> {
        let zone = match below_4g {
            true => Zone::Below4G,
            false => Zone::Any,
        };
        let memory = self.host.dma(pages, zone).ok()?;
        // SAFETY: the host gave this zeroed, page-aligned, contiguous memory to
        // this driver alone, reachable by the device at `device()`, and keeps it.
        Some(unsafe { Dma::new(memory.device(), memory.cpu(), memory.len()) })
    }

    fn read32(&self, offset: usize) -> u32 {
        self.regs.read32(offset as u64)
    }

    fn write32(&self, offset: usize, value: u32) {
        self.regs.write32(offset as u64, value);
    }

    fn now_ns(&self) -> u64 {
        self.host.now_ns()
    }

    fn interrupts(&self) -> bool {
        self.interrupts
    }

    fn delay(&self, ns: u64) {
        self.host.delay(ns);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ptr::NonNull;
    use devkit::DmaError;
    use std::boxed::Box;
    use std::vec::Vec;

    /// A host over plain memory: one register window, pages from the heap.
    struct FakeHost {
        registers: Box<[u32; 16]>,
        pages: Vec<Box<[u8; 4096]>>,
        zones: Vec<Zone>,
    }

    impl Host for FakeHost {
        fn dma(&mut self, pages: usize, zone: Zone) -> Result<devkit::Dma, DmaError> {
            if pages != 1 {
                return Err(DmaError::NoMemory);
            }
            self.zones.push(zone);
            let mut page = Box::new([0u8; 4096]);
            let cpu = NonNull::new(page.as_mut_ptr()).unwrap();
            let device = 0x1000 * self.pages.len() as u64 + 0x8000_0000;
            self.pages.push(page);
            // SAFETY: the page lives in `self.pages` for the test's length.
            Ok(unsafe { devkit::Dma::new(cpu, device, 4096) })
        }
        fn mmio(&mut self, _: u8, _: u64, len: u64) -> Result<Mmio, DmaError> {
            let base = NonNull::new(self.registers.as_mut_ptr()).unwrap();
            // SAFETY: the registers live in `self` for the test's length.
            Ok(unsafe { Mmio::new(base, len.min(64)) })
        }
        fn irqs(&mut self, _: u16) -> u16 {
            0
        }
        fn wait(&mut self, _: u64, _: u64) -> u64 {
            0
        }
        fn cfg_read(&self, _: u16) -> u32 {
            u32::MAX
        }
        fn cfg_write(&mut self, _: u16, _: u32) {}
        fn now_ns(&self) -> u64 {
            42
        }
    }

    #[test]
    fn the_adapter_passes_dma_registers_and_time_through() {
        let mut host = FakeHost {
            registers: Box::new([0; 16]),
            pages: Vec::new(),
            zones: Vec::new(),
        };
        let regs = host.mmio(0, 0, 64).unwrap();
        let mut hal = HostHal::new(host, regs, true);
        let low = hal.dma(1, true).unwrap();
        let any = hal.dma(1, false).unwrap();
        assert_eq!((low.phys(), any.phys()), (0x8000_0000, 0x8000_1000));
        assert!(
            hal.dma(2, false).is_none(),
            "the host's refusal is passed on"
        );
        assert_eq!(hal.host.zones, [Zone::Below4G, Zone::Any]);
        hal.write32(8, 7);
        assert_eq!(hal.read32(8), 7);
        assert_eq!(hal.read32(64), u32::MAX, "outside the window");
        assert_eq!(
            (hal.now_ns(), hal.interrupts(), hal.mapped()),
            (42, true, 64)
        );
    }
}
