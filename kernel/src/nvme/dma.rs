//! Physically contiguous memory for the NVMe driver: queue pages, the Identify
//! page, PRP lists and staging buffers. Each is allocated once, zeroed, reached
//! through the direct map and never freed: a controller lives as long as the boot.

use crate::memory;
use mm::PAGE_SIZE;

/// `pages` contiguous frames.
pub struct Page {
    pub phys: u64,
    pub virt: u64,
    len: usize,
}

impl Page {
    pub fn new() -> Result<Self, &'static str> {
        Self::contiguous(1)
    }

    pub fn contiguous(pages: u64) -> Result<Self, &'static str> {
        let phys = memory::with(|m| m.allocate_contiguous(pages))
            .ok_or("nvme: out of frames for device memory")?;
        let virt = memory::hhdm() + phys;
        let len = (pages * PAGE_SIZE) as usize;
        // SAFETY: the frames were just allocated for this driver alone and the direct
        // map covers them.
        unsafe { core::ptr::write_bytes(virt as *mut u8, 0, len) };
        Ok(Self { phys, virt, len })
    }

    /// The first `len` bytes, as the device last wrote them.
    pub fn bytes(&self, len: usize) -> &[u8] {
        let len = len.min(self.len);
        // SAFETY: the memory is ours for good and `len` stays inside it; the device
        // writes it only while a command runs, and commands are waited on.
        unsafe { core::slice::from_raw_parts(self.virt as *const u8, len) }
    }

    /// The first `len` bytes, to fill before a command that reads them.
    pub fn bytes_mut(&mut self, len: usize) -> &mut [u8] {
        let len = len.min(self.len);
        // SAFETY: as in `bytes`; `&mut self` keeps any other view away meanwhile.
        unsafe { core::slice::from_raw_parts_mut(self.virt as *mut u8, len) }
    }

    /// Writes `value` as the `index`-th u64.
    pub fn set_u64(&mut self, index: usize, value: u64) {
        assert!(
            (index + 1) * 8 <= self.len,
            "nvme: a write past device memory"
        );
        // SAFETY: checked just above to lie inside the memory, which is 8-aligned.
        unsafe { ((self.virt as *mut u64).add(index)).write_volatile(value) };
    }
}

// SAFETY: a Page is plain memory owned by the driver; which CPU touches it does not
// matter, and the block list's mutex keeps it to one at a time.
unsafe impl Send for Page {}
