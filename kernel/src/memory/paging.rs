//! Page-table operations on top of Limine's tables, reached through the HHDM.

use mm::{FrameBitmap, PAGE_SIZE};
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::mapper::TranslateResult;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
    Translate,
};
use x86_64::{PhysAddr, VirtAddr};

/// First PML4 slot of the kernel half.
const KERNEL_HALF_FIRST_SLOT: usize = 256;

pub struct Memory {
    pub(super) frames: FrameBitmap<'static>,
    pub(super) mapper: OffsetPageTable<'static>,
    pub(super) hhdm: u64,
    /// Physical address of the kernel PML4 (Limine's), which kernel threads run on.
    pub(super) kernel_pml4: u64,
}

/// Feeds new page-table frames to the x86_64 crate's mapper, zeroed.
pub(super) struct Frames<'a> {
    pub(super) bitmap: &'a mut FrameBitmap<'static>,
    pub(super) hhdm: u64,
}

// SAFETY: hands out frames the bitmap marked used, so each is unique and unused.
unsafe impl FrameAllocator<Size4KiB> for Frames<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let number = self.bitmap.allocate()?;
        let frame = PhysFrame::containing_address(PhysAddr::new(number * PAGE_SIZE));
        zero_frame(self.hhdm, frame);
        Some(frame)
    }
}

pub(super) fn zero_frame(hhdm: u64, frame: PhysFrame) {
    let ptr = (hhdm + frame.start_address().as_u64()) as *mut u8;
    // SAFETY: the frame is usable RAM, mapped by the HHDM, and owned by the caller.
    unsafe { core::ptr::write_bytes(ptr, 0, PAGE_SIZE as usize) };
}

fn page(addr: u64) -> Page<Size4KiB> {
    Page::containing_address(VirtAddr::new(addr))
}

impl Memory {
    /// # Safety
    /// CR3 must hold Limine's live page tables and `hhdm` must be Limine's HHDM offset.
    pub unsafe fn adopt(frames: FrameBitmap<'static>, hhdm: u64) -> Self {
        let (l4_frame, _) = Cr3::read();
        let l4 = (hhdm + l4_frame.start_address().as_u64()) as *mut PageTable;
        // SAFETY: per the contract, l4 is the active PML4 mapped through the HHDM.
        let mapper = unsafe { OffsetPageTable::new(&mut *l4, VirtAddr::new(hhdm)) };
        Self {
            kernel_pml4: l4_frame.start_address().as_u64(),
            frames,
            mapper,
            hhdm,
        }
    }

    pub fn free_frames(&self) -> u64 {
        self.frames.free_count()
    }

    pub fn total_frames(&self) -> u64 {
        self.frames.total()
    }

    /// Gives every kernel-half PML4 slot a table, so future address spaces can share
    /// the kernel half by copying slots 256..512. Returns how many were created.
    pub fn prepare_kernel_half(&mut self) -> Result<usize, &'static str> {
        let mut created = 0;
        for slot in KERNEL_HALF_FIRST_SLOT..512 {
            if !self.mapper.level_4_table()[slot].is_unused() {
                continue;
            }
            let number = self
                .frames
                .allocate()
                .ok_or("out of frames for kernel-half tables")?;
            let frame = PhysFrame::containing_address(PhysAddr::new(number * PAGE_SIZE));
            zero_frame(self.hhdm, frame);
            self.mapper.level_4_table_mut()[slot]
                .set_frame(frame, PageTableFlags::PRESENT | PageTableFlags::WRITABLE);
            created += 1;
        }
        Ok(created)
    }

    pub fn kernel_half_complete(&self) -> bool {
        let l4 = self.mapper.level_4_table();
        (KERNEL_HALF_FIRST_SLOT..512).all(|slot| !l4[slot].is_unused())
    }

    /// Maps a fresh zeroed frame at `addr`.
    pub fn map(&mut self, addr: u64, flags: PageTableFlags) -> Result<(), &'static str> {
        let number = self.frames.allocate().ok_or("out of frames")?;
        let frame = PhysFrame::containing_address(PhysAddr::new(number * PAGE_SIZE));
        zero_frame(self.hhdm, frame);
        let mut tables = Frames {
            bitmap: &mut self.frames,
            hhdm: self.hhdm,
        };
        // SAFETY: the frame is freshly allocated and the page is in a kernel-owned range.
        match unsafe { self.mapper.map_to(page(addr), frame, flags, &mut tables) } {
            Ok(flush) => {
                flush.flush();
                Ok(())
            }
            Err(_) => {
                let _ = self.frames.free(number);
                Err("page already mapped or table allocation failed")
            }
        }
    }

    /// Maps the existing physical page `phys` at `addr`, for device registers and
    /// firmware tables. The frame is not owned by the allocator and is never freed.
    pub fn map_physical_page(
        &mut self,
        addr: u64,
        phys: u64,
        flags: PageTableFlags,
    ) -> Result<(), &'static str> {
        let frame = PhysFrame::containing_address(PhysAddr::new(phys));
        let mut tables = Frames {
            bitmap: &mut self.frames,
            hhdm: self.hhdm,
        };
        // SAFETY: the caller maps device or firmware memory into a kernel-owned window.
        let flush = unsafe { self.mapper.map_to(page(addr), frame, flags, &mut tables) }
            .map_err(|_| "physical window page already mapped")?;
        flush.flush();
        Ok(())
    }

    /// Unmaps `addr` and returns its frame to the allocator.
    pub fn unmap(&mut self, addr: u64) -> Result<(), &'static str> {
        let frame = self.unmap_keep_frame(addr)?;
        self.frames
            .free(frame / PAGE_SIZE)
            .map_err(|_| "unmapped frame was not allocated")
    }

    /// Unmaps `addr` everywhere and returns the physical frame without freeing it.
    pub fn unmap_keep_frame(&mut self, addr: u64) -> Result<u64, &'static str> {
        let (frame, flush) = self
            .mapper
            .unmap(page(addr))
            .map_err(|_| "page not mapped")?;
        flush.flush();
        crate::smp::tlb_shootdown(addr);
        Ok(frame.start_address().as_u64())
    }

    /// Points a mapped page at a different physical frame. Returns the old frame,
    /// which the caller now owns.
    pub fn retarget(
        &mut self,
        addr: u64,
        phys: u64,
        flags: PageTableFlags,
    ) -> Result<u64, &'static str> {
        let old = self.unmap_keep_frame(addr)?;
        self.map_physical_page(addr, phys, flags)?;
        Ok(old)
    }

    /// Returns a frame that a caller of `unmap_keep_frame` or `retarget` owns.
    /// Allocates `count` physically contiguous frames; returns the first physical address.
    pub fn allocate_contiguous(&mut self, count: u64) -> Option<u64> {
        self.frames
            .allocate_contiguous(count, 1)
            .map(|frame| frame * PAGE_SIZE)
    }

    /// Allocates `count` physically contiguous frames whose first frame number is a
    /// multiple of `align` (512 for a 2 MiB run), not zeroed; returns the first
    /// physical address.
    pub fn allocate_aligned(&mut self, count: u64, align: u64) -> Option<u64> {
        self.frames
            .allocate_contiguous(count, align)
            .map(|frame| frame * PAGE_SIZE)
    }

    /// Allocates one zeroed frame; returns its physical address.
    pub fn allocate_zeroed_frame(&mut self) -> Option<u64> {
        let number = self.frames.allocate()?;
        let frame = PhysFrame::containing_address(PhysAddr::new(number * PAGE_SIZE));
        zero_frame(self.hhdm, frame);
        Some(number * PAGE_SIZE)
    }

    pub fn free_contiguous(&mut self, phys: u64, count: u64) -> Result<(), &'static str> {
        self.frames
            .free_contiguous(phys / PAGE_SIZE, count)
            .map_err(|_| "contiguous frames were not allocated")
    }

    /// Whether the frame at `phys` is allocated.
    pub fn frame_in_use(&self, phys: u64) -> bool {
        !self.frames.is_free(phys / PAGE_SIZE)
    }

    pub fn free_frame(&mut self, phys: u64) -> Result<(), &'static str> {
        self.frames
            .free(phys / PAGE_SIZE)
            .map_err(|_| "frame was not allocated")
    }

    /// Replaces the leaf flags of a mapped page on every CPU.
    pub fn protect(&mut self, addr: u64, flags: PageTableFlags) -> Result<(), &'static str> {
        // SAFETY: callers only change pages they mapped themselves.
        let flush = unsafe { self.mapper.update_flags(page(addr), flags) }
            .map_err(|_| "page not mapped")?;
        flush.flush();
        crate::smp::tlb_shootdown(addr);
        Ok(())
    }

    /// Physical address and leaf flags of a mapped address.
    pub fn translate(&self, addr: u64) -> Option<(u64, PageTableFlags)> {
        match self.mapper.translate(VirtAddr::new(addr)) {
            TranslateResult::Mapped {
                frame,
                offset,
                flags,
            } => Some((frame.start_address().as_u64() + offset, flags)),
            _ => None,
        }
    }

    /// Reports whether the CPU touched the page since the last call, and clears the bit.
    pub fn take_accessed(&mut self, addr: u64) -> Option<bool> {
        let (_, flags) = self.translate(addr)?;
        if !flags.contains(PageTableFlags::ACCESSED) {
            return Some(false);
        }
        self.protect(addr, flags - PageTableFlags::ACCESSED).ok()?;
        Some(true)
    }
}
