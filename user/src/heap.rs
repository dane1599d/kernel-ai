//! The program heap: talc, growing the heap with `brk` whenever it runs out,
//! and for allocations of `kabi::heap::BIG_BYTES` or more a mapping of their
//! own, given back to the system when freed.
//!
//! talc's heap never shrinks, and the kernel kills a process whose `brk` goes
//! past its page quota as the call returns, so nothing can be logged after it.
//! So the heap logs the allocation that grows it past each
//! `kabi::heap::MARK_BYTES`, and a big allocation the quota refuses
//! (MEM_COMMIT says NoMemory, with no kill) is logged and fails.

use crate::sys;
use core::alloc::{GlobalAlloc, Layout};
use core::fmt::Write;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};
use kabi::heap::{BigTable, Cached, LineBuf, big_bytes, passed_mark};
use talc::base::Talc;
use talc::base::binning::Binning;
use talc::source::Source;
use talc::sync::TalcLock;

/// Smallest step the heap grows by.
const GROW_STEP: u64 = 64 * 1024;
/// A log line's most bytes.
const LINE_BYTES: usize = 160;

#[derive(Debug)]
pub struct Brk {
    start: u64,
    end: Option<NonNull<u8>>,
}

// SAFETY: `Brk` lives inside the allocator's lock (TalcLock over a spin mutex), so
// with worker threads (L6) only one thread at a time touches it, and the kernel
// grows the heap one BRK call at a time; nothing in it is tied to a thread.
unsafe impl Send for Brk {}

/// Writes `args` to the console as one line, with no allocation.
fn log(args: core::fmt::Arguments) {
    let mut line = LineBuf::<LINE_BYTES>::new();
    let _ = line.write_fmt(args);
    let _ = line.write_str("\n");
    let _ = sys::write(line.as_bytes());
}

// SAFETY: acquire only makes system calls and uses the `talc` it is given.
unsafe impl Source for Brk {
    fn acquire<B: Binning>(talc: &mut Talc<Self, B>, layout: Layout) -> Result<(), ()> {
        let want = (layout.size() + layout.align() + 256) as u64;
        let grow = want.div_ceil(GROW_STEP) * GROW_STEP;
        let old_end = sys::brk(0).map_err(|_| ())?;
        if talc.source.end.is_none() {
            talc.source.start = old_end;
        }
        let start = talc.source.start;
        // Before the call: past the quota the kernel kills the process as it
        // returns, so this line is the last word on what grew it.
        if let Some(mib) = passed_mark(old_end - start, old_end - start + grow) {
            log(format_args!(
                "heap: growing past {mib} MiB for a {}-byte allocation",
                layout.size()
            ));
        }
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
    Allocator::new(Brk {
        start: 0,
        end: None,
    })
}

/// What a big allocation came to.
enum Big {
    /// Its own mapping, at this address.
    Mapped(*mut u8),
    /// None to be had (the table or the kernel's reservations are full): the
    /// heap takes it.
    Heap,
    /// The page quota refused it.
    Refused,
}

/// The global allocator: talc and the big allocations' own mappings, counting
/// the bytes in use so a program can watch its own heap (the shell stops a line
/// that uses too much).
pub struct Counting {
    inner: Allocator,
    big: spin::Mutex<BigTable>,
    /// Freed big mappings kept for an allocation of the same size, so a
    /// buffer made and dropped every frame costs no system calls.
    cached: spin::Mutex<Cached>,
    live: AtomicU64,
}

impl Counting {
    pub const fn new() -> Self {
        Self {
            inner: new(),
            big: spin::Mutex::new(BigTable::new()),
            cached: spin::Mutex::new(Cached::new()),
            live: AtomicU64::new(0),
        }
    }

    pub fn live_bytes(&self) -> u64 {
        self.live.load(Ordering::Relaxed)
    }

    /// Gives every kept mapping back to the system: an app that naps
    /// (comphost) returns what it no longer draws with.
    pub fn trim(&self) {
        let kept = self.cached.lock().take_all();
        for (start, _) in kept.into_iter().flatten() {
            let _ = sys::mem_release(start);
        }
    }

    /// `bytes` (whole pages) of their own for an allocation of `size`; zeroed
    /// unless a kept one of that size was reused (`reused`).
    fn map_big(&self, bytes: u64, size: usize, reused: &mut bool) -> Big {
        let mut table = self.big.lock();
        if !table.has_room() {
            return Big::Heap;
        }
        if let Some(start) = self.cached.lock().take(bytes) {
            table.insert(start);
            *reused = true;
            return Big::Mapped(start as *mut u8);
        }
        let Ok(start) = sys::mem_reserve(bytes) else {
            return Big::Heap;
        };
        if let Err(error) = sys::mem_commit(start, bytes) {
            let _ = sys::mem_release(start);
            log(format_args!(
                "heap: a {size}-byte allocation did not fit the page quota ({error:?}, {} bytes live)",
                self.live_bytes()
            ));
            return Big::Refused;
        }
        table.insert(start);
        Big::Mapped(start as *mut u8)
    }

    /// Takes `ptr` back if it is a big allocation's of `bytes`: kept for the
    /// next of its size, or given back to the system; whether it was one.
    fn unmap_big(&self, ptr: *mut u8, bytes: u64) -> bool {
        let start = ptr as u64;
        let mut table = self.big.lock();
        if !table.remove(start) {
            return false;
        }
        if let Some((gone, _)) = self.cached.lock().put(start, bytes) {
            let _ = sys::mem_release(gone);
        }
        true
    }

    fn count(&self, ptr: *mut u8, size: usize) -> *mut u8 {
        if !ptr.is_null() {
            self.live.fetch_add(size as u64, Ordering::Relaxed);
        }
        ptr
    }

    /// The allocation, `zeroed` or not: a big one's pages come zeroed.
    unsafe fn allocate(&self, layout: Layout, zeroed: bool) -> *mut u8 {
        if let Some(bytes) = big_bytes(layout.size(), layout.align()) {
            let mut reused = false;
            match self.map_big(bytes, layout.size(), &mut reused) {
                Big::Mapped(ptr) => {
                    if zeroed && reused {
                        // SAFETY: the mapping is this allocation's, `bytes` long.
                        unsafe { core::ptr::write_bytes(ptr, 0, layout.size()) };
                    }
                    return self.count(ptr, layout.size());
                }
                Big::Refused => return core::ptr::null_mut(),
                Big::Heap => {}
            }
        }
        // SAFETY: the caller upholds GlobalAlloc's contract.
        let ptr = unsafe {
            match zeroed {
                true => GlobalAlloc::alloc_zeroed(&self.inner, layout),
                false => GlobalAlloc::alloc(&self.inner, layout),
            }
        };
        self.count(ptr, layout.size())
    }
}

impl Default for Counting {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: forwards every call to talc or to the kernel's reservations, which
// hand out page-aligned, zeroed memory of at least the size asked for, and only
// adds bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds GlobalAlloc's contract.
        unsafe { self.allocate(layout, false) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as above.
        unsafe { self.allocate(layout, true) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let big = big_bytes(layout.size(), layout.align())
            .is_some_and(|bytes| self.unmap_big(ptr, bytes));
        if !big {
            // SAFETY: as above; `ptr` is not a big allocation's, so talc's.
            unsafe { GlobalAlloc::dealloc(&self.inner, ptr, layout) };
        }
        self.live.fetch_sub(layout.size() as u64, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let was_big = big_bytes(layout.size(), layout.align()).is_some()
            && self.big.lock().contains(ptr as u64);
        let goes_big = big_bytes(new_size, layout.align()).is_some();
        if !was_big && !goes_big {
            // SAFETY: as above; talc's allocation stays talc's.
            let new = unsafe { GlobalAlloc::realloc(&self.inner, ptr, layout, new_size) };
            if !new.is_null() {
                self.live.fetch_add(new_size as u64, Ordering::Relaxed);
                self.live.fetch_sub(layout.size() as u64, Ordering::Relaxed);
            }
            return new;
        }
        // Into or out of a mapping of its own: a new allocation, the bytes
        // copied, the old one freed.
        // SAFETY: the caller guarantees `new_size` with `layout`'s alignment is valid.
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        // SAFETY: as above.
        let new = unsafe { self.allocate(new_layout, false) };
        if !new.is_null() {
            // SAFETY: both are live, distinct, and at least this long.
            unsafe { core::ptr::copy_nonoverlapping(ptr, new, layout.size().min(new_size)) };
            // SAFETY: `ptr` was allocated with `layout` and is not used again.
            unsafe { self.dealloc(ptr, layout) };
        }
        new
    }
}
