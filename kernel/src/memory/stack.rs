//! Kernel stacks with an unmapped guard page below them, so an overflow faults
//! instead of silently corrupting whatever lies below.

use super::STACKS_START;
use mm::PAGE_SIZE;
use x86_64::structures::paging::PageTableFlags;

/// Virtual space reserved per stack slot: guard page plus up to 63 stack pages.
const SLOT_SIZE: u64 = 64 * PAGE_SIZE;

static NEXT_SLOT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Frames every guarded stack took, page tables included; stacks are never freed.
static FRAMES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Frames all guarded stacks have taken since boot, so a test can tell a new
/// thread slot's stack from a leak.
pub fn frames_taken() -> u64 {
    FRAMES.load(core::sync::atomic::Ordering::SeqCst)
}

pub struct GuardedStack {
    /// The unmapped page directly below the stack.
    pub guard: u64,
    /// One past the highest stack byte; the initial RSP.
    pub top: u64,
}

impl GuardedStack {
    /// Maps `pages` stack pages above an unmapped guard page. Stacks are never freed yet.
    pub fn new(pages: u64) -> Result<Self, &'static str> {
        if pages == 0 || pages >= SLOT_SIZE / PAGE_SIZE {
            return Err("stack size out of range");
        }
        let slot = NEXT_SLOT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let guard = STACKS_START + slot * SLOT_SIZE;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
        super::with(|memory| {
            let before = memory.free_frames();
            let mapped = (1..=pages).try_for_each(|i| memory.map(guard + i * PAGE_SIZE, flags));
            let taken = before - memory.free_frames();
            FRAMES.fetch_add(taken, core::sync::atomic::Ordering::SeqCst);
            mapped
        })?;
        Ok(Self {
            guard,
            top: guard + (pages + 1) * PAGE_SIZE,
        })
    }
}
