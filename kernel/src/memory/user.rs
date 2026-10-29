//! Ring-3 address spaces: one PML4 per process. The upper half (slots 256..512) is
//! copied from the kernel PML4 and shared, since every kernel-half PML4 entry was
//! created at boot and never changes. The lower half belongs to the process.
//!
//! TLB rules, which is why nothing here sends a shootdown:
//! - Kernel threads always run on the kernel PML4 (no lazy CR3), so only CPUs running
//!   one of the process's threads (its main thread and, since L6, its workers) have
//!   its CR3 loaded.
//! - User pages are never GLOBAL and PCIDs are off, so any CR3 write flushes them.
//! - Pages are only ever added while the process lives (not-present entries are never
//!   cached), never unmapped, and the space is freed only by its last thread out,
//!   after every one of its threads switched its CPU to the kernel PML4.
//! - The one exception is shared pages (`Shm`, ADR 0001 D4): `crate::shm` unmaps or
//!   retargets them while the process runs, and shoots the range down on every
//!   CPU itself. They are never OWNED, so teardown leaves their frames alone.
//!
//! Every frame a space uses, page tables included, is counted against the process's
//! page quota and in [`USER_FRAMES`], which must return to zero when every process
//! is gone. The one exception is the model's window (L6): a page-table subtree the
//! kernel built once, linked into the model's PML4 slot with one entry by
//! [`Memory::link_window`]. It is the kernel's, counts nowhere, maps 2 MiB pages, and
//! is unlinked, never freed, when the space is destroyed, so `free_table` never meets
//! it. No other user mapping is ever made in that slot.

use super::paging::{Frames, Memory};
use core::sync::atomic::{AtomicU64, Ordering};
use kabi::layout::USER_END;
use mm::PAGE_SIZE;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

/// Lowest address a process may map: page 0 and its neighbours stay unmapped.
pub const MIN_USER_ADDRESS: u64 = 0x1_0000;
/// Marks leaf entries whose frames the process owns and teardown must free.
const OWNED: PageTableFlags = PageTableFlags::BIT_9;
/// Frames held by all user address spaces together.
pub static USER_FRAMES: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, PartialEq, Eq)]
pub struct UserSpace {
    pml4: u64,
    /// Frames this space holds: the PML4, page tables and mapped pages.
    frames: u64,
    /// The window linked in, if any: its PML4 slot and its PDPT.
    window: Option<(usize, u64)>,
    /// Pages counted against the quota that the space does not hold: the `Shm`
    /// objects the process made.
    charged: u64,
    /// Its anonymous reservations (MEM_RESERVE), kept under the space's lock.
    pub reservations: mm::reserve::Reservations,
}

/// The PML4 slot of the model's window, where no other user page may be mapped.
const WINDOW_SLOT: usize = (kabi::layout::MODEL_BASE >> 39) as usize;

impl UserSpace {
    /// Physical address of the PML4, for CR3.
    pub fn cr3(&self) -> u64 {
        self.pml4
    }

    /// Frames counted against the page quota: those it holds and those charged.
    pub fn frames(&self) -> u64 {
        self.frames + self.charged
    }

    /// Counts `pages` it does not hold against the quota, if they fit under
    /// `max_pages` with everything counted already; whether they did.
    pub fn try_charge(&mut self, pages: u64, max_pages: u64) -> bool {
        let fits = self.frames().saturating_add(pages) <= max_pages;
        if fits {
            self.charged += pages;
        }
        fits
    }

    /// Takes back pages [`UserSpace::try_charge`] counted.
    pub fn refund(&mut self, pages: u64) {
        self.charged = self.charged.saturating_sub(pages);
    }
}

/// Permissions of a user page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    pub write: bool,
    pub execute: bool,
}

/// Counts the page-table frames the mapper allocates.
struct Counting<'a> {
    inner: Frames<'a>,
    allocated: u64,
}

// SAFETY: forwards to the zeroing frame allocator.
unsafe impl FrameAllocator<Size4KiB> for Counting<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = self.inner.allocate_frame()?;
        self.allocated += 1;
        Some(frame)
    }
}

/// A page a process may own: in user space, and not in the model window's slot.
fn is_user_page(addr: u64) -> bool {
    addr.is_multiple_of(PAGE_SIZE)
        && (MIN_USER_ADDRESS..USER_END).contains(&addr)
        && (addr >> 39) as usize != WINDOW_SLOT
}

impl Memory {
    fn table(&self, phys: u64) -> &'static mut PageTable {
        // SAFETY: page-table frames are usable RAM mapped by the HHDM, and are only
        // touched under the MEMORY lock.
        unsafe { &mut *((self.hhdm + phys) as *mut PageTable) }
    }

    /// A new address space sharing the kernel half.
    pub fn new_user_space(&mut self) -> Result<UserSpace, &'static str> {
        let number = self.frames.allocate().ok_or("out of frames")?;
        let pml4 = number * PAGE_SIZE;
        super::paging::zero_frame(
            self.hhdm,
            PhysFrame::containing_address(PhysAddr::new(pml4)),
        );
        let kernel = self.table(self.kernel_pml4);
        let table = self.table(pml4);
        for slot in 256..512 {
            table[slot] = kernel[slot].clone();
        }
        USER_FRAMES.fetch_add(1, Ordering::Relaxed);
        Ok(UserSpace {
            pml4,
            frames: 1,
            window: None,
            charged: 0,
            reservations: mm::reserve::Reservations::new(
                kabi::layout::MEM_BASE,
                kabi::layout::MEM_END,
            ),
        })
    }

    /// Maps a fresh zeroed page at `addr` in `space`; returns its physical address.
    /// Refuses anything outside the user range, so a user-chosen address can never
    /// reach the shared kernel half.
    pub fn map_user_page(
        &mut self,
        space: &mut UserSpace,
        addr: u64,
        access: Access,
    ) -> Result<u64, &'static str> {
        if !is_user_page(addr) {
            return Err("not a user page address");
        }
        let number = self.frames.allocate().ok_or("out of frames")?;
        let frame = PhysFrame::containing_address(PhysAddr::new(number * PAGE_SIZE));
        super::paging::zero_frame(self.hhdm, frame);
        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | OWNED;
        if access.write {
            flags |= PageTableFlags::WRITABLE;
        }
        if !access.execute {
            flags |= PageTableFlags::NO_EXECUTE;
        }
        let parents =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
        let mut tables = Counting {
            inner: Frames {
                bitmap: &mut self.frames,
                hhdm: self.hhdm,
            },
            allocated: 0,
        };
        let hhdm = self.hhdm;
        // SAFETY: the PML4 belongs to `space` and is reached through the HHDM.
        let mut mapper = unsafe {
            OffsetPageTable::new(
                &mut *((hhdm + space.pml4) as *mut PageTable),
                VirtAddr::new(hhdm),
            )
        };
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
        // SAFETY: the frame is fresh and the page is in the user half of this space.
        // Not flushed: not-present entries are never cached.
        let mapped =
            unsafe { mapper.map_to_with_table_flags(page, frame, flags, parents, &mut tables) };
        let new_tables = tables.allocated;
        space.frames += new_tables;
        USER_FRAMES.fetch_add(new_tables, Ordering::Relaxed);
        match mapped {
            Ok(flush) => {
                flush.ignore();
                space.frames += 1;
                USER_FRAMES.fetch_add(1, Ordering::Relaxed);
                Ok(frame.start_address().as_u64())
            }
            Err(_) => {
                let _ = self.frames.free(number);
                Err("page already mapped or out of frames for page tables")
            }
        }
    }

    /// Maps the frame at `phys`, which `space` does not own (an `Shm` page), at
    /// `addr`: read-only or read-write, never executable. Only the page tables it
    /// adds count against the space, and teardown leaves the frame alone.
    pub fn map_shared_page(
        &mut self,
        space: &mut UserSpace,
        addr: u64,
        phys: u64,
        write: bool,
    ) -> Result<(), &'static str> {
        if !is_user_page(addr) {
            return Err("not a user page address");
        }
        let frame = PhysFrame::containing_address(PhysAddr::new(phys));
        let mut flags =
            PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::NO_EXECUTE;
        if write {
            flags |= PageTableFlags::WRITABLE;
        }
        let parents =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
        let mut tables = Counting {
            inner: Frames {
                bitmap: &mut self.frames,
                hhdm: self.hhdm,
            },
            allocated: 0,
        };
        let hhdm = self.hhdm;
        // SAFETY: the PML4 belongs to `space` and is reached through the HHDM.
        let mut mapper = unsafe {
            OffsetPageTable::new(
                &mut *((hhdm + space.pml4) as *mut PageTable),
                VirtAddr::new(hhdm),
            )
        };
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
        // SAFETY: the frame is the object's and the page is in this space's user half.
        // Not flushed: not-present entries are never cached.
        let mapped =
            unsafe { mapper.map_to_with_table_flags(page, frame, flags, parents, &mut tables) };
        space.frames += tables.allocated;
        USER_FRAMES.fetch_add(tables.allocated, Ordering::Relaxed);
        mapped
            .map(|flush| flush.ignore())
            .map_err(|_| "page already mapped or out of frames for page tables")
    }

    /// Points the shared page at `addr` in `space` at `phys`, its flags kept, or
    /// removes it with `None`. Refuses a page the space owns. Not flushed: the
    /// caller shoots the range down.
    pub fn set_shared_page(
        &mut self,
        space: &UserSpace,
        addr: u64,
        phys: Option<u64>,
    ) -> Result<(), &'static str> {
        self.set_shared_range(space, addr, 1, phys)
    }

    /// [`Self::set_shared_page`] for `count` pages from `addr`, every one pointed
    /// at the same `phys` (a revoked mapping's zero page) or removed: one walk
    /// from the PML4 per leaf table, not per page. A page that is not a shared
    /// one is left alone and makes the result an error.
    pub fn set_shared_range(
        &mut self,
        space: &UserSpace,
        addr: u64,
        count: u64,
        phys: Option<u64>,
    ) -> Result<(), &'static str> {
        let mut result = Ok(());
        let mut done = 0;
        while done < count {
            let at = addr + done * PAGE_SIZE;
            let first = ((at >> 12) & 0x1FF) as usize;
            let run = (512 - first).min((count - done) as usize);
            done += run as u64;
            let leaf = match self.leaf_table(space, at) {
                Ok(leaf) => leaf,
                Err(error) => {
                    result = Err(error);
                    continue;
                }
            };
            let entries = self.table(leaf).iter_mut().skip(first).take(run);
            for (n, entry) in entries.enumerate() {
                let flags = entry.flags();
                // Never the model window's slot or the kernel half, whatever the caller.
                if !is_user_page(at + n as u64 * PAGE_SIZE)
                    || !flags.contains(PageTableFlags::PRESENT)
                    || flags.contains(OWNED)
                {
                    result = Err("no shared page there");
                    continue;
                }
                match phys {
                    Some(phys) => entry.set_addr(PhysAddr::new(phys), flags),
                    None => entry.set_unused(),
                }
            }
        }
        result
    }

    /// Unmaps the pages `space` owns in `[addr, addr + count pages)` and moves
    /// their frames into `out`, which must have room for `count` (it never
    /// grows here, under the MEMORY lock). The frames stay allocated: the caller
    /// shoots the range down on every CPU first, then frees them with
    /// [`Memory::free_user_frames`], so no stale TLB entry reaches a reused
    /// frame, and a process that exits meanwhile can neither free them twice
    /// nor leak them. Pages not mapped or not owned are skipped.
    pub fn take_owned_range(
        &mut self,
        space: &mut UserSpace,
        addr: u64,
        count: u64,
        out: &mut alloc::vec::Vec<u64>,
    ) {
        let mut done = 0;
        while done < count {
            let at = addr + done * PAGE_SIZE;
            let first = ((at >> 12) & 0x1FF) as usize;
            let run = (512 - first).min((count - done) as usize);
            done += run as u64;
            let Ok(leaf) = self.leaf_table(space, at) else {
                continue;
            };
            for entry in self.table(leaf).iter_mut().skip(first).take(run) {
                let flags = entry.flags();
                if !flags.contains(PageTableFlags::PRESENT) || !flags.contains(OWNED) {
                    continue;
                }
                if out.len() == out.capacity() {
                    // Never allocate here; the caller sized `out` from the book.
                    continue;
                }
                out.push(entry.addr().as_u64() / PAGE_SIZE);
                entry.set_unused();
                space.frames -= 1;
                USER_FRAMES.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    /// Frees frame numbers [`Memory::take_owned_range`] took, after their
    /// shootdown.
    pub fn free_user_frames(&mut self, frames: &[u64]) {
        for &number in frames {
            let _ = self.frames.free(number);
        }
    }

    /// The leaf page table that maps `addr` in `space`.
    fn leaf_table(&self, space: &UserSpace, addr: u64) -> Result<u64, &'static str> {
        if !is_user_page(addr) {
            return Err("not a user page address");
        }
        let indexes = [
            (addr >> 39) & 0x1FF,
            (addr >> 30) & 0x1FF,
            (addr >> 21) & 0x1FF,
        ];
        let mut table = space.pml4;
        for index in indexes {
            let entry = &self.table(table)[index as usize];
            let flags = entry.flags();
            if !flags.contains(PageTableFlags::PRESENT) || flags.contains(PageTableFlags::HUGE_PAGE)
            {
                return Err("no shared page there");
            }
            table = entry.addr().as_u64();
        }
        Ok(table)
    }

    /// Maps the firmware page `phys` at `addr` in `space`, present, writable and
    /// executable, for kernel use only: `efi.rs`'s temporary PML4 for
    /// SetVirtualAddressMap, which maps the runtime regions 1:1. Never OWNED, so
    /// tearing the space down leaves the firmware's frame alone.
    pub fn map_firmware_page(
        &mut self,
        space: &mut UserSpace,
        addr: u64,
        phys: u64,
    ) -> Result<(), &'static str> {
        if !is_user_page(addr) {
            return Err("not a lower-half page address");
        }
        let frame = PhysFrame::containing_address(PhysAddr::new(phys));
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        let mut tables = Counting {
            inner: Frames {
                bitmap: &mut self.frames,
                hhdm: self.hhdm,
            },
            allocated: 0,
        };
        let hhdm = self.hhdm;
        // SAFETY: the PML4 belongs to `space` and is reached through the HHDM.
        let mut mapper = unsafe {
            OffsetPageTable::new(
                &mut *((hhdm + space.pml4) as *mut PageTable),
                VirtAddr::new(hhdm),
            )
        };
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
        // SAFETY: the frame is firmware memory the space never owns, in its lower half.
        let mapped =
            unsafe { mapper.map_to_with_table_flags(page, frame, flags, flags, &mut tables) };
        space.frames += tables.allocated;
        USER_FRAMES.fetch_add(tables.allocated, Ordering::Relaxed);
        mapped
            .map(|flush| flush.ignore())
            .map_err(|_| "page already mapped or out of frames for page tables")
    }

    /// Links the window whose PDPT is at `pdpt` (built by the kernel, read-only and
    /// no-execute at every level) into `space`'s PML4 slot holding `base`: one entry,
    /// no frame taken. Linking it again changes nothing; a slot holding anything
    /// else is refused. Not flushed: a not-present entry is never cached.
    pub fn link_window(
        &mut self,
        space: &mut UserSpace,
        base: u64,
        pdpt: u64,
    ) -> Result<(), &'static str> {
        let slot = (base >> 39) as usize;
        if slot != WINDOW_SLOT || !base.is_multiple_of(1 << 39) {
            return Err("not the model window's slot");
        }
        let link = mm::window::link(pdpt).map_err(|_| "the window's PDPT is misplaced")?;
        let entry = &mut self.table(space.pml4)[slot];
        let raw = entry.flags().bits() | entry.addr().as_u64();
        if mm::window::links(raw, pdpt) {
            return Ok(());
        }
        if !entry.is_unused() {
            return Err("the model window's slot holds something else");
        }
        // `link` holds the PDPT's address and the window's table flags.
        entry.set_addr(
            PhysAddr::new(link & mm::window::ADDRESS),
            PageTableFlags::from_bits_retain(link & !mm::window::ADDRESS),
        );
        space.window = Some((slot, pdpt));
        Ok(())
    }

    /// Physical address and flags of a user address in `space`. A large page (the
    /// model's window) reads as unmapped, so no system call takes a pointer into it.
    pub fn translate_user(&self, space: &UserSpace, addr: u64) -> Option<(u64, PageTableFlags)> {
        if addr >= USER_END {
            return None;
        }
        let indexes = [
            (addr >> 39) & 0x1FF,
            (addr >> 30) & 0x1FF,
            (addr >> 21) & 0x1FF,
            (addr >> 12) & 0x1FF,
        ];
        let mut table = space.pml4;
        for (level, index) in indexes.into_iter().enumerate() {
            let entry = &self.table(table)[index as usize];
            let flags = entry.flags();
            if !flags.contains(PageTableFlags::PRESENT) || flags.contains(PageTableFlags::HUGE_PAGE)
            {
                return None;
            }
            if level == 3 {
                return Some((entry.addr().as_u64() + (addr & (PAGE_SIZE - 1)), flags));
            }
            table = entry.addr().as_u64();
        }
        None
    }

    /// Frees every page and page table of `space`. The caller has made sure no CPU
    /// has its CR3 loaded. Returns how many frames were freed. The model's window is
    /// unlinked first, never freed.
    pub fn destroy_user_space(&mut self, space: UserSpace) -> u64 {
        if let Some((slot, pdpt)) = space.window {
            let entry = &mut self.table(space.pml4)[slot];
            let raw = entry.flags().bits() | entry.addr().as_u64();
            if mm::window::links(raw, pdpt) {
                entry.set_unused();
            }
        }
        let freed = self.free_table(space.pml4, 4, 0..256);
        USER_FRAMES.fetch_sub(freed, Ordering::Relaxed);
        if freed != space.frames {
            crate::log!(
                "memory: user space held {} frames but {freed} were freed",
                space.frames
            );
        }
        freed
    }

    /// Frees the subtree under the table at `phys` (level 4 = PML4) and the table.
    /// An entry mapping a large page is never descended or freed: only the kernel's
    /// window has them, and it is unlinked before this runs.
    fn free_table(&mut self, phys: u64, level: u32, slots: core::ops::Range<usize>) -> u64 {
        let mut freed = 0;
        for slot in slots {
            let entry = self.table(phys)[slot].clone();
            let flags = entry.flags();
            if !flags.contains(PageTableFlags::PRESENT) {
                continue;
            }
            if level > 1 && flags.contains(PageTableFlags::HUGE_PAGE) {
                crate::log!("memory: a user space held a large page; left alone");
                continue;
            }
            let child = entry.addr().as_u64();
            if level == 1 {
                if flags.contains(OWNED) && self.frames.free(child / PAGE_SIZE).is_ok() {
                    freed += 1;
                }
            } else {
                freed += self.free_table(child, level - 1, 0..512);
            }
        }
        if self.frames.free(phys / PAGE_SIZE).is_ok() {
            freed += 1;
        }
        freed
    }

    pub fn kernel_cr3(&self) -> u64 {
        self.kernel_pml4
    }
}
