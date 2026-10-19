//! Physical frames, page tables, the kernel heap and guarded kernel stacks.
//!
//! Virtual layout of the kernel half (PML4 slots 256..512, shared by every future
//! address space):
//!
//! | Range start             | Use                                   |
//! |-------------------------|---------------------------------------|
//! | `0xffff_8000_0000_0000` | Limine's higher-half direct map (HHDM) |
//! | `0xffff_c000_0000_0000` | kernel heap                           |
//! | `0xffff_c800_0000_0000` | kernel stacks with guard pages        |
//! | `0xffff_d000_0000_0000` | self-test scratch mappings            |
//! | `0xffff_d800_0000_0000` | device registers and firmware tables  |
//! | `0xffff_ffff_8000_0000` | kernel image (Limine)                 |

pub mod heap;
pub mod paging;
pub mod selftest;
pub mod stack;
pub mod user;

use crate::log;
use limine::memmap::{Entry, MEMMAP_USABLE};
use mm::{FrameBitmap, PAGE_SIZE};
use paging::Memory;
use spin::Mutex;
use x86_64::structures::paging::PageTableFlags;

pub const HEAP_START: u64 = 0xFFFF_C000_0000_0000;
pub const STACKS_START: u64 = 0xFFFF_C800_0000_0000;
pub const SCRATCH_START: u64 = 0xFFFF_D000_0000_0000;
pub const PHYS_WINDOW_START: u64 = 0xFFFF_D800_0000_0000;

static NEXT_WINDOW_PAGE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Maps `len` bytes of physical memory starting at `phys` into the device window and
/// returns the virtual address of `phys`. Mappings are permanent.
pub fn map_physical(phys: u64, len: u64, flags: PageTableFlags) -> Result<u64, &'static str> {
    let first = phys / PAGE_SIZE * PAGE_SIZE;
    let pages = (phys + len.max(1)).div_ceil(PAGE_SIZE) - first / PAGE_SIZE;
    let start = NEXT_WINDOW_PAGE.fetch_add(pages, core::sync::atomic::Ordering::Relaxed);
    let base = PHYS_WINDOW_START + start * PAGE_SIZE;
    with(|m| {
        (0..pages).try_for_each(|i| {
            m.map_physical_page(base + i * PAGE_SIZE, first + i * PAGE_SIZE, flags)
        })
    })?;
    Ok(base + (phys - first))
}

static MEMORY: Mutex<Option<Memory>> = Mutex::new(None);
static HHDM_OFFSET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Physical address of the kernel PML4, readable without the MEMORY lock.
static KERNEL_CR3: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// End of the tracked physical memory, readable without the MEMORY lock.
static PHYS_END: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The page tables kernel threads run on.
pub fn kernel_cr3() -> u64 {
    KERNEL_CR3.load(core::sync::atomic::Ordering::Relaxed)
}

/// Offset of Limine's higher-half direct map: physical address + hhdm() is mapped.
pub fn hhdm() -> u64 {
    HHDM_OFFSET.load(core::sync::atomic::Ordering::Relaxed)
}

/// End of the physical memory the frame allocator tracks: every frame it hands out
/// lies below this, and is reachable at `hhdm() + phys`.
pub fn phys_end() -> u64 {
    PHYS_END.load(core::sync::atomic::Ordering::Relaxed)
}

/// Runs `f` with the memory manager. Panics if called before [`init`].
///
/// Never call this with interrupts off once the scheduler runs: the holder may be
/// waiting for a TLB shootdown that a CPU spinning here with interrupts off would
/// never acknowledge.
pub fn with<R>(f: impl FnOnce(&mut Memory) -> R) -> R {
    debug_assert!(
        !crate::threads::running() || x86_64::instructions::interrupts::are_enabled(),
        "memory::with called with interrupts off"
    );
    crate::threads::preempt_disable();
    let mut guard = MEMORY.lock();
    let result = f(guard.as_mut().expect("memory::init has run"));
    drop(guard);
    crate::threads::preempt_enable();
    result
}

/// Builds the frame allocator from Limine's memory map, adopts Limine's page tables,
/// prepares the shared kernel half, and maps the heap.
pub fn init(entries: &[&Entry], hhdm: u64) -> Result<(), &'static str> {
    let usable = || entries.iter().filter(|e| e.type_ == MEMMAP_USABLE);
    let end = usable()
        .map(|e| e.base + e.length)
        .max()
        .ok_or("no usable memory")?;
    let frames = end / PAGE_SIZE;
    let storage_bytes = (FrameBitmap::words_for(frames) * 8) as u64;
    let storage_region = usable()
        .find(|e| e.base >= PAGE_SIZE && e.length >= storage_bytes)
        .ok_or("no usable region large enough for the frame bitmap")?;
    // SAFETY: the region is usable RAM, mapped by Limine's HHDM, and reserved below
    // so nothing else will ever hand it out.
    let storage = unsafe {
        core::slice::from_raw_parts_mut(
            (hhdm + storage_region.base) as *mut u64,
            FrameBitmap::words_for(frames),
        )
    };
    let mut bitmap = FrameBitmap::new(storage, frames);
    for entry in usable() {
        bitmap.release_range(entry.base / PAGE_SIZE, entry.length / PAGE_SIZE);
    }
    bitmap.reserve_range(
        storage_region.base / PAGE_SIZE,
        storage_bytes.div_ceil(PAGE_SIZE),
    );
    bitmap.reserve_range(0, 1);

    // SAFETY: Limine's page tables are live in CR3 and reachable through the HHDM.
    let mut memory = unsafe { Memory::adopt(bitmap, hhdm) };
    HHDM_OFFSET.store(hhdm, core::sync::atomic::Ordering::Relaxed);
    KERNEL_CR3.store(memory.kernel_cr3(), core::sync::atomic::Ordering::Relaxed);
    PHYS_END.store(frames * PAGE_SIZE, core::sync::atomic::Ordering::Relaxed);
    let prepared = memory.prepare_kernel_half()?;
    heap::map(&mut memory)?;
    let (free, total) = (memory.free_frames(), memory.total_frames());
    *MEMORY.lock() = Some(memory);
    log!(
        "memory: {} MiB free of {} MiB tracked, {prepared} kernel-half tables prepared, heap {} MiB",
        free * PAGE_SIZE / (1024 * 1024),
        total * PAGE_SIZE / (1024 * 1024),
        heap::HEAP_SIZE / (1024 * 1024)
    );
    Ok(())
}
