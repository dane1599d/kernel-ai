//! Per-thread class labels and the standby slot.
//!
//! A trial needs to swap policies in and out from the timer tick, where nothing
//! may allocate (building a policy allocates a `Box`). So the candidate is built
//! beforehand and staged in the standby slot with its labels; `flip` swaps it with
//! the installed policy, `revert` swaps back to the last good one, and `commit`
//! keeps the candidate and hands back the old policy for the caller to drop
//! outside interrupt-disabled code. None of the three allocates.
//!
//! Labels belong to a slot: the installed policy has a table and a staged
//! candidate brings its own, and `flip` and `revert` swap them with the policies.
//! `set_class` labels the installed policy's table. Every policy that is
//! installed gets its table replayed, so a fresh policy and a reverted one see the
//! labels they were installed with. A person's `set_policy` during a trial ends
//! it and restores the labels from before the trial. The kernel keeps one Core
//! per CPU and sets labels on each.

use super::{Core, State};
use crate::classes::Class;
use crate::{EnqueueReason, MAX_THREADS, SchedPolicy, Tid};
use alloc::boxed::Box;

impl Core {
    /// Labels a thread in the installed policy's table.
    pub fn set_class(&mut self, tid: Tid, class: Class) {
        let Some(slot) = self.classes.get_mut(tid) else {
            return;
        };
        *slot = class;
        self.policy.set_class(tid, class);
    }

    /// Forgets a thread's label in both slots: for a thread slot that is reused,
    /// on every Core, since a thread may later migrate to any of them.
    pub fn clear_class(&mut self, tid: Tid) {
        if tid >= MAX_THREADS {
            return;
        }
        self.classes[tid] = Class::Normal;
        self.standby_classes[tid] = Class::Normal;
        self.policy.set_class(tid, Class::Normal);
    }

    /// The installed policy's labels.
    pub fn class_table(&self) -> [Class; MAX_THREADS] {
        self.classes
    }

    pub fn class(&self, tid: Tid) -> Class {
        self.classes.get(tid).copied().unwrap_or_default()
    }

    /// Stages `candidate` and the labels it should run with. Returns whatever was
    /// in the slot before, for the caller to drop; refuses (handing the candidate
    /// back) while a trial runs.
    #[allow(clippy::type_complexity)]
    pub fn stage(
        &mut self,
        candidate: Box<dyn SchedPolicy>,
        classes: [Class; MAX_THREADS],
    ) -> Result<Option<Box<dyn SchedPolicy>>, Box<dyn SchedPolicy>> {
        if self.in_trial {
            return Err(candidate);
        }
        self.standby_classes = classes;
        self.staged = true;
        Ok(self.standby.replace(candidate))
    }

    /// Installs the staged candidate at `now`; the installed policy becomes the
    /// standby, the last good one. False if nothing is staged, a trial already
    /// runs, or the installed policy has been ejected (it is not good to return to).
    pub fn flip(&mut self, now: u64) -> bool {
        if !self.staged || self.in_trial || self.on_fallback || self.standby.is_none() {
            return false;
        }
        self.swap_standby(now);
        self.in_trial = true;
        true
    }

    /// Puts the last good policy back at `now`, also after an ejection; the
    /// candidate stays staged and can be flipped in again. False if no trial runs.
    pub fn revert(&mut self, now: u64) -> bool {
        if !self.in_trial {
            return false;
        }
        self.swap_standby(now);
        self.in_trial = false;
        true
    }

    /// Keeps the candidate and returns the old policy, to be dropped outside
    /// interrupt-disabled code. `None` if no trial runs, or if the watchdog ejected
    /// the candidate: that trial can only be reverted.
    pub fn commit(&mut self) -> Option<Box<dyn SchedPolicy>> {
        if !self.in_trial || self.on_fallback {
            return None;
        }
        self.in_trial = false;
        self.staged = false;
        self.standby.take()
    }

    /// Empties the standby slot (outside a trial), for the caller to drop.
    pub fn take_standby(&mut self) -> Option<Box<dyn SchedPolicy>> {
        if self.in_trial {
            return None;
        }
        self.staged = false;
        self.standby.take()
    }

    pub fn in_trial(&self) -> bool {
        self.in_trial
    }

    pub fn standby_name(&self) -> Option<&'static str> {
        self.standby.as_ref().map(|policy| policy.name())
    }

    fn swap_standby(&mut self, now: u64) {
        let Some(standby) = self.standby.as_mut() else {
            return;
        };
        core::mem::swap(&mut self.policy, standby);
        core::mem::swap(&mut self.classes, &mut self.standby_classes);
        // The policy coming back may still hold the queue it had when it left.
        self.policy.reset();
        self.installed_at = now;
        self.install_active();
    }

    /// Replays the labels into the installed policy, leaves the fallback, hands
    /// the policy every runnable thread, and tells it that the running thread
    /// woke, if it did, so it keeps the shield a woken thread has.
    pub(super) fn install_active(&mut self) {
        for (tid, class) in self.classes.iter().enumerate() {
            self.policy.set_class(tid, *class);
        }
        self.on_fallback = false;
        self.fallback.reset();
        self.requeue_runnable_into_active();
        let current = self.current;
        if current != self.idle && self.states[current] == State::Running && self.woke[current] {
            // Queued and taken out again: the flags stay, the queue does not.
            self.policy.enqueue(current, EnqueueReason::Woken);
            self.policy.dequeue(current);
        }
    }
}

#[cfg(test)]
mod tests;
