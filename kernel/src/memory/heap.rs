//! Kernel heap: a fixed, pre-mapped region managed by talc, behind a counting wrapper
//! so tests can prove that allocations are not leaked.

use super::HEAP_START;
use super::paging::Memory;
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};
use mm::PAGE_SIZE;
use talc::DefaultBinning;
use talc::source::Claim;
use talc::sync::TalcLock;
use x86_64::structures::paging::PageTableFlags;

pub const HEAP_SIZE: u64 = 32 * 1024 * 1024;

type Allocator = TalcLock<spin::mutex::SpinMutex<()>, Claim, DefaultBinning>;

// SAFETY: the heap range is mapped by `map` before the first allocation, and nothing
// else uses it.
static TALC: Allocator =
    Allocator::new(unsafe { Claim::new(HEAP_START as *mut u8, HEAP_SIZE as usize) });

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// Live allocation count and bytes, for leak checks.
pub fn live() -> (usize, usize) {
    (
        LIVE_ALLOCATIONS.load(Ordering::SeqCst),
        LIVE_BYTES.load(Ordering::SeqCst),
    )
}

/// Maps the whole heap range writable and non-executable.
pub fn map(memory: &mut Memory) -> Result<(), &'static str> {
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
    (0..HEAP_SIZE / PAGE_SIZE).try_for_each(|i| memory.map(HEAP_START + i * PAGE_SIZE, flags))
}

struct Counting;

/// Runs `f`, which takes talc's lock, with preemption off: talc's lock is a
/// plain spinlock, and a thread switched out while it held it left every other
/// CPU that wanted memory spinning, some of them with preemption off, so the
/// holder could never run again (the `tune-revert` hang, 2026-10-27).
fn unpreempted<R>(f: impl FnOnce() -> R) -> R {
    crate::threads::preempt_disable();
    let result = f();
    crate::threads::preempt_enable();
    result
}

// SAFETY: forwards to talc and only adds bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let ptr = unpreempted(|| unsafe { TALC.alloc(layout) });
        if !ptr.is_null() {
            LIVE_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            LIVE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: same contract as the caller's.
        unpreempted(|| unsafe { TALC.dealloc(ptr, layout) });
        LIVE_ALLOCATIONS.fetch_sub(1, Ordering::Relaxed);
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let new = unpreempted(|| unsafe { TALC.realloc(ptr, layout, new_size) });
        if !new.is_null() {
            LIVE_BYTES.fetch_add(new_size, Ordering::Relaxed);
            LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        new
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;
