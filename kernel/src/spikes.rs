//! M0 spike: prove Rhai and wasmi build for x86_64-unknown-none and run on bare metal.
//! Uses a throwaway bump allocator; the real heap arrives in M2.

use crate::log;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

const ARENA_SIZE: usize = 8 * 1024 * 1024;

#[repr(C, align(4096))]
struct Arena(UnsafeCell<[u8; ARENA_SIZE]>);

// SAFETY: the arena is only handed out in disjoint pieces by BumpAllocator.
unsafe impl Sync for Arena {}

static ARENA: Arena = Arena(UnsafeCell::new([0; ARENA_SIZE]));

/// Never frees memory. Good enough for a single boot-time spike on one CPU.
struct BumpAllocator {
    next: AtomicUsize,
}

// SAFETY: hands out non-overlapping, correctly aligned ranges of ARENA, or null when full.
unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = ARENA.0.get() as usize;
        let mut current = self.next.load(Ordering::Relaxed);
        loop {
            let start = (base + current).next_multiple_of(layout.align()) - base;
            let end = match start.checked_add(layout.size()) {
                Some(end) if end <= ARENA_SIZE => end,
                _ => return core::ptr::null_mut(),
            };
            match self
                .next
                .compare_exchange(current, end, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return (base + start) as *mut u8,
                Err(actual) => current = actual,
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    next: AtomicUsize::new(0),
};

/// Runs both spikes and logs their results. Returns an error message on failure.
pub fn run() -> Result<(), &'static str> {
    match spike_rhai::run() {
        Ok(42) => log!("spike-rhai: ok (script returned 42)"),
        Ok(other) => {
            log!("spike-rhai: wrong result {}", other);
            return Err("Rhai spike returned the wrong value");
        }
        Err(error) => {
            log!("spike-rhai: error {}", error);
            return Err("Rhai spike failed");
        }
    }
    match spike_wasmi::run() {
        Ok(42) => log!("spike-wasmi: ok (add(40, 2) returned 42)"),
        Ok(other) => {
            log!("spike-wasmi: wrong result {}", other);
            return Err("wasmi spike returned the wrong value");
        }
        Err(error) => {
            log!("spike-wasmi: error {}", error);
            return Err("wasmi spike failed");
        }
    }
    log!(
        "spikes: arena used {} KiB",
        ALLOCATOR.next.load(Ordering::Relaxed) / 1024
    );
    Ok(())
}
