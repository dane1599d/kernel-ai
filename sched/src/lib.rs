//! Scheduler logic, independent of hardware so it can run in a host simulator.
//!
//! The split follows Linux's sched_ext: [`Core`] is the mechanism (thread states,
//! sleeping, waking, the watchdog), and a [`SchedPolicy`] only decides which runnable
//! thread goes next. A policy that misbehaves is ejected and [`RoundRobin`] takes over.
//!
//! Policies run in interrupt context, so they must not allocate, block or take locks.
//! The fixed-capacity [`Ring`] exists for that reason.

#![no_std]

extern crate alloc;

pub mod burst;
mod core_state;
mod policy;

pub use burst::BurstTable;
pub use core_state::{Core, State};
pub use policy::{Interactive, RoundRobin};

/// Thread identifier: an index into fixed-size tables.
pub type Tid = usize;

/// Maximum number of threads, including the idle thread.
pub const MAX_THREADS: usize = 64;

/// The idle thread. It is never queued; it runs when nothing else can.
pub const IDLE: Tid = 0;

/// Why a thread is being put back in the run queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueReason {
    New,
    Woken,
    Preempted,
    Yielded,
}

pub trait SchedPolicy: Send {
    fn name(&self) -> &'static str;

    /// A thread became runnable.
    fn enqueue(&mut self, tid: Tid, reason: EnqueueReason);

    /// A queued thread stopped being runnable without being picked.
    fn dequeue(&mut self, tid: Tid);

    /// Removes and returns the thread to run next, or `None` to idle.
    fn pick_next(&mut self) -> Option<Tid>;

    /// Called every tick for the running thread. `slice_ns` is how long it has run
    /// since it was picked. Return true to preempt it.
    fn tick(&mut self, current: Tid, slice_ns: u64) -> bool;

    /// Whether waking `woken` should preempt `current` immediately.
    fn preempt_on_wake(&self, _woken: Tid, _current: Tid) -> bool {
        false
    }

    /// Forgets every queued thread.
    fn reset(&mut self);
}

/// Fixed-capacity FIFO of thread IDs; never allocates.
#[derive(Clone)]
pub struct Ring {
    items: [Tid; MAX_THREADS],
    head: usize,
    len: usize,
}

impl Default for Ring {
    fn default() -> Self {
        Self {
            items: [0; MAX_THREADS],
            head: 0,
            len: 0,
        }
    }
}

impl Ring {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Appends `tid`. Returns false if the ring is full or already holds it.
    pub fn push_back(&mut self, tid: Tid) -> bool {
        if self.len == MAX_THREADS || self.contains(tid) {
            return false;
        }
        self.items[(self.head + self.len) % MAX_THREADS] = tid;
        self.len += 1;
        true
    }

    pub fn pop_front(&mut self) -> Option<Tid> {
        if self.len == 0 {
            return None;
        }
        let tid = self.items[self.head];
        self.head = (self.head + 1) % MAX_THREADS;
        self.len -= 1;
        Some(tid)
    }

    pub fn contains(&self, tid: Tid) -> bool {
        (0..self.len).any(|i| self.items[(self.head + i) % MAX_THREADS] == tid)
    }

    /// Removes `tid` if present, keeping the order of the rest.
    pub fn remove(&mut self, tid: Tid) -> bool {
        let Some(position) =
            (0..self.len).find(|&i| self.items[(self.head + i) % MAX_THREADS] == tid)
        else {
            return false;
        };
        for i in position..self.len - 1 {
            self.items[(self.head + i) % MAX_THREADS] =
                self.items[(self.head + i + 1) % MAX_THREADS];
        }
        self.len -= 1;
        true
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_is_fifo_without_duplicates() {
        let mut ring = Ring::default();
        assert!(ring.push_back(3) && ring.push_back(5) && ring.push_back(7));
        assert!(!ring.push_back(5));
        assert!(ring.remove(5));
        assert!(!ring.remove(9));
        assert_eq!(ring.pop_front(), Some(3));
        assert_eq!(ring.pop_front(), Some(7));
        assert_eq!(ring.pop_front(), None);
    }

    #[test]
    fn ring_wraps_and_fills() {
        let mut ring = Ring::default();
        for round in 0..3 {
            for tid in 0..MAX_THREADS {
                assert!(ring.push_back(tid), "round {round}");
            }
            assert!(!ring.push_back(MAX_THREADS + 1));
            for tid in 0..MAX_THREADS {
                assert_eq!(ring.pop_front(), Some(tid));
            }
        }
        assert!(ring.is_empty());
    }
}
