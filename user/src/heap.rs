//! The program heap: talc, growing the heap with `brk` whenever it runs out.

use crate::sys;
use core::alloc::Layout;
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
