//! Device interrupt vectors (ADR 0007 section 4): 0x30-0xEF, handed out one per
//! MSI-X entry. The fixed vectors in `apic` stay where they are. A handler only
//! acknowledges its device and says which thread to wake; driver work does not
//! run here. A vector nobody owns is counted and dropped, not fatal: until an
//! IOMMU remaps interrupts, a device can raise any vector.

use crate::{apic, threads};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub const FIRST: u8 = 0x30;
pub const LAST: u8 = 0xEF;
const COUNT: usize = (LAST - FIRST) as usize + 1;

/// Runs with interrupts off on the CPU the vector was delivered to; returns a
/// thread to wake once the interrupt is acknowledged.
pub type Handler = fn() -> Option<usize>;

/// Each vector's handler as an address, 0 while free.
static HANDLERS: [AtomicUsize; COUNT] = [const { AtomicUsize::new(0) }; COUNT];
static COUNTS: [AtomicU64; COUNT] = [const { AtomicU64::new(0) }; COUNT];
static STRAY: AtomicU64 = AtomicU64::new(0);

/// A free vector, now owned by `handler`; `None` when all are taken.
pub fn allocate(handler: Handler) -> Option<u8> {
    let address = handler as usize;
    let index = HANDLERS.iter().position(|slot| {
        slot.compare_exchange(0, address, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    })?;
    Some(FIRST + index as u8)
}

/// Called from `trap_dispatch` for a vector in `FIRST..=LAST`.
pub fn on_interrupt(vector: u8) {
    let index = usize::from(vector.saturating_sub(FIRST)).min(COUNT - 1);
    let address = HANDLERS[index].load(Ordering::Acquire);
    let waiter = if address == 0 {
        STRAY.fetch_add(1, Ordering::Relaxed);
        None
    } else {
        COUNTS[index].fetch_add(1, Ordering::Relaxed);
        // SAFETY: only `allocate` stores a nonzero value, and it stores a `Handler`.
        let handler: Handler = unsafe { core::mem::transmute::<usize, Handler>(address) };
        handler()
    };
    apic::end_of_interrupt();
    if let Some(tid) = waiter {
        threads::wake(tid);
    }
}

/// Interrupts `vector` has taken.
pub fn count(vector: u8) -> u64 {
    match vector.checked_sub(FIRST).map(usize::from) {
        Some(index) if index < COUNT => COUNTS[index].load(Ordering::Relaxed),
        _ => 0,
    }
}

/// Interrupts on vectors nobody owns.
pub fn stray() -> u64 {
    STRAY.load(Ordering::Relaxed)
}
