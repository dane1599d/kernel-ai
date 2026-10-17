//! The program heap: talc, growing the heap with `brk` whenever it runs out.

use crate::sys;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use talc::base::Talc;
use talc::base::binning::Binning;
use talc::source::Source;
use talc::sync::TalcLock;

/// Smallest step the heap grows by.
const GROW_STEP: u64 = 64 * 1024;

#[derive(Debug)]
pub struct Brk {
    end: Option<NonNull<u8>>,
}

// SAFETY: a ring-3 program has one thread, and nothing is shared with another.
unsafe impl Send for Brk {}

// SAFETY: acquire only makes system calls and uses the `talc` it is given.
unsafe impl Source for Brk {
    fn acquire<B: Binning>(talc: &mut Talc<Self, B>, layout: Layout) -> Result<(), ()> {
        let want = (layout.size() + layout.align() + 256) as u64;
        let grow = want.div_ceil(GROW_STEP) * GROW_STEP;
        let old_end = sys::brk(0).map_err(|_| ())?;
        let new_end = sys::brk(grow).map_err(|_| ())?;
        let end = match talc.source.end {
            // SAFETY: the kernel just mapped [old_end, new_end) for this program alone.
            None => unsafe { talc.claim(old_end as *mut u8, (new_end - old_end) as usize) },
            // SAFETY: as above; the new pages directly follow the current heap end.
            Some(end) => Some(unsafe { talc.extend(end, new_end as *mut u8) }),
        };
        talc.source.end = end;
        end.map(|_| ()).ok_or(())
    }
}

pub type Allocator = TalcLock<spin::mutex::SpinMutex<()>, Brk, talc::DefaultBinning>;

pub const fn new() -> Allocator {
    Allocator::new(Brk { end: None })
}

/// The global allocator: talc, counting the bytes in use so a program can watch
/// its own heap (the shell stops a line that uses too much).
pub struct Counting {
    inner: Allocator,
    live: core::sync::atomic::AtomicU64,
}

impl Counting {
    pub const fn new() -> Self {
        Self {
            inner: new(),
            live: core::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn live_bytes(&self) -> u64 {
        self.live.load(core::sync::atomic::Ordering::Relaxed)
    }
}

impl Default for Counting {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: forwards every call to talc and only adds bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds GlobalAlloc's contract.
        let ptr = unsafe { GlobalAlloc::alloc(&self.inner, layout) };
        if !ptr.is_null() {
            self.live
                .fetch_add(layout.size() as u64, core::sync::atomic::Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as above.
        unsafe { GlobalAlloc::dealloc(&self.inner, ptr, layout) };
        self.live
            .fetch_sub(layout.size() as u64, core::sync::atomic::Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: as above.
        let new = unsafe { GlobalAlloc::realloc(&self.inner, ptr, layout, new_size) };
        if !new.is_null() {
            self.live
                .fetch_add(new_size as u64, core::sync::atomic::Ordering::Relaxed);
            self.live
                .fetch_sub(layout.size() as u64, core::sync::atomic::Ordering::Relaxed);
        }
        new
    }
}
