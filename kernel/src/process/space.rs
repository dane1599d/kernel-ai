//! The address-space lock, held with preemption off (see [`SpaceGuard`]).

use super::UserSpace;
use core::sync::atomic::Ordering;

/// A locked address space, with preemption off until it is dropped. SPACES is a
/// spinlock: a holder preempted while holding it could be kept off every CPU by
/// the threads spinning for it (an llm with 8 workers in FUTEX_WAIT did exactly
/// that under VirtualBox). No holder sleeps, so none needs preemption. Its drop
/// leaves what the space counts against the quota in the slot's `HELD`, for
/// the admission rule (bug hunt 5 H5-17).
pub(crate) struct SpaceGuard {
    slot: usize,
    /// Always Some until `drop` releases it.
    guard: Option<spin::MutexGuard<'static, Option<UserSpace>>>,
}

/// Locks process `slot`'s address space; see [`SpaceGuard`].
pub(crate) fn lock_space(slot: usize) -> SpaceGuard {
    crate::threads::ready::not_in_ready(&"an address space");
    crate::threads::preempt_disable();
    SpaceGuard {
        slot,
        guard: Some(super::SPACES[slot].lock()),
    }
}

impl core::ops::Deref for SpaceGuard {
    type Target = Option<UserSpace>;
    fn deref(&self) -> &Option<UserSpace> {
        self.guard
            .as_ref()
            .expect("a SpaceGuard holds its lock until dropped")
    }
}

impl core::ops::DerefMut for SpaceGuard {
    fn deref_mut(&mut self) -> &mut Option<UserSpace> {
        self.guard
            .as_mut()
            .expect("a SpaceGuard holds its lock until dropped")
    }
}

impl Drop for SpaceGuard {
    fn drop(&mut self) {
        // While nobody else can change it; none once the space is gone. Stored
        // only on a change: most locks (a uaccess copy) change nothing, and
        // eight slots share a cache line.
        let held = match self.guard.as_deref() {
            Some(Some(space)) => space.frames(),
            _ => 0,
        };
        let published = &super::HELD[self.slot];
        if published.load(Ordering::Relaxed) != held {
            published.store(held, Ordering::Relaxed);
        }
        // Unlock first: preemption comes back on only once nobody can wait on us.
        drop(self.guard.take());
        crate::threads::preempt_enable();
    }
}
