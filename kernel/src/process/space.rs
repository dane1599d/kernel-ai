//! The address-space lock, held with preemption off (see [`SpaceGuard`]).

use super::UserSpace;

/// A locked address space, with preemption off until it is dropped. SPACES is a
/// spinlock: a holder preempted while holding it could be kept off every CPU by
/// the threads spinning for it (an llm with 8 workers in FUTEX_WAIT did exactly
/// that under VirtualBox). No holder sleeps, so none needs preemption.
pub(crate) struct SpaceGuard {
    /// Always Some until `drop` releases it.
    guard: Option<spin::MutexGuard<'static, Option<UserSpace>>>,
}

/// Locks process `slot`'s address space; see [`SpaceGuard`].
pub(crate) fn lock_space(slot: usize) -> SpaceGuard {
    crate::threads::preempt_disable();
    SpaceGuard {
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
        // Unlock first: preemption comes back on only once nobody can wait on us.
        drop(self.guard.take());
        crate::threads::preempt_enable();
    }
}
