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
    frames: FrameBitmap<'static>,
    mapper: OffsetPageTable<'static>,
    hhdm: u64,
}

/// Feeds new page-table frames to the x86_64 crate's mapper, zeroed.
struct Frames<'a> {
    bitmap: &'a mut FrameBitmap<'static>,
    hhdm: u64,
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

fn zero_frame(hhdm: u64, frame: PhysFrame) {
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

    /// Unmaps `addr` and returns its frame to the allocator.
    pub fn unmap(&mut self, addr: u64) -> Result<(), &'static str> {
        let (frame, flush) = self
            .mapper
            .unmap(page(addr))
            .map_err(|_| "page not mapped")?;
        flush.flush();
        self.frames
            .free(frame.start_address().as_u64() / PAGE_SIZE)
            .map_err(|_| "unmapped frame was not allocated")
    }

    /// Replaces the leaf flags of a mapped page.
    pub fn protect(&mut self, addr: u64, flags: PageTableFlags) -> Result<(), &'static str> {
        // SAFETY: callers only change pages they mapped themselves.
        let flush = unsafe { self.mapper.update_flags(page(addr), flags) }
            .map_err(|_| "page not mapped")?;
        flush.flush();
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
