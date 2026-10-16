//! Built-in policies. Both are allocation-free and constant-size.

use crate::{EnqueueReason, MAX_THREADS, Ring, SchedPolicy, Tid};

const MS: u64 = 1_000_000;

/// FIFO with a fixed time slice. The fallback after an ejection, so it must stay simple.
pub struct RoundRobin {
    queue: Ring,
    slice_ns: u64,
}

impl Default for RoundRobin {
    fn default() -> Self {
        Self {
            queue: Ring::default(),
            slice_ns: 10 * MS,
        }
    }
}

impl RoundRobin {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SchedPolicy for RoundRobin {
    fn name(&self) -> &'static str {
        "round-robin"
    }

    fn enqueue(&mut self, tid: Tid, _reason: EnqueueReason) {
        self.queue.push_back(tid);
    }

    fn dequeue(&mut self, tid: Tid) {
        self.queue.remove(tid);
    }

    fn pick_next(&mut self) -> Option<Tid> {
        self.queue.pop_front()
    }

    fn tick(&mut self, _current: Tid, slice_ns: u64) -> bool {
        slice_ns >= self.slice_ns && !self.queue.is_empty()
    }

    fn reset(&mut self) {
        self.queue.clear();
    }
}

/// Latency-first policy: threads that just woke (from a key press, a timer, I/O)
/// run before threads that used up their slice, and preempt them on wake. Woken
/// threads get a short slice, so a thread that keeps computing drops back to the
/// normal queue. CPU-bound threads share the rest round-robin.
pub struct Interactive {
    woken: Ring,
    normal: Ring,
    boosted: [bool; MAX_THREADS],
    normal_slice_ns: u64,
    boosted_slice_ns: u64,
}

impl Default for Interactive {
    fn default() -> Self {
        Self {
            woken: Ring::default(),
            normal: Ring::default(),
            boosted: [false; MAX_THREADS],
            normal_slice_ns: 10 * MS,
            boosted_slice_ns: 2 * MS,
        }
    }
}

impl Interactive {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SchedPolicy for Interactive {
    fn name(&self) -> &'static str {
        "interactive"
    }

    fn enqueue(&mut self, tid: Tid, reason: EnqueueReason) {
        let Some(flag) = self.boosted.get_mut(tid) else {
            return;
        };
        *flag = reason == EnqueueReason::Woken;
        if *flag {
            self.woken.push_back(tid);
        } else {
            self.normal.push_back(tid);
        }
    }

    fn dequeue(&mut self, tid: Tid) {
        self.woken.remove(tid);
        self.normal.remove(tid);
    }

    fn pick_next(&mut self) -> Option<Tid> {
        self.woken.pop_front().or_else(|| self.normal.pop_front())
    }

    fn tick(&mut self, current: Tid, slice_ns: u64) -> bool {
        let boosted = self.boosted.get(current).copied().unwrap_or(false);
        let limit = if boosted {
            self.boosted_slice_ns
        } else {
            self.normal_slice_ns
        };
        slice_ns >= limit && !(self.woken.is_empty() && self.normal.is_empty())
    }

    fn preempt_on_wake(&self, _woken: Tid, current: Tid) -> bool {
        !self.boosted.get(current).copied().unwrap_or(false)
    }

    fn reset(&mut self) {
        self.woken.clear();
        self.normal.clear();
        self.boosted = [false; MAX_THREADS];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_robin_rotates_after_its_slice() {
        let mut rr = RoundRobin::new();
        rr.enqueue(1, EnqueueReason::New);
        rr.enqueue(2, EnqueueReason::New);
        assert_eq!(rr.pick_next(), Some(1));
        assert!(!rr.tick(1, 5 * MS));
        assert!(rr.tick(1, 10 * MS));
        rr.enqueue(1, EnqueueReason::Preempted);
        assert_eq!(rr.pick_next(), Some(2));
    }

    #[test]
    fn interactive_runs_woken_threads_first_and_preempts_for_them() {
        let mut policy = Interactive::new();
        policy.enqueue(1, EnqueueReason::New);
        policy.enqueue(2, EnqueueReason::New);
        assert_eq!(policy.pick_next(), Some(1));
        policy.enqueue(3, EnqueueReason::Woken);
        assert!(policy.preempt_on_wake(3, 1));
        policy.enqueue(1, EnqueueReason::Preempted);
        assert_eq!(policy.pick_next(), Some(3));
        // A boosted thread is not preempted by another wake-up...
        assert!(!policy.preempt_on_wake(4, 3));
        // ...and loses its boost after a short slice.
        assert!(policy.tick(3, 2 * MS));
        policy.enqueue(3, EnqueueReason::Preempted);
        assert_eq!(policy.pick_next(), Some(2));
    }
}
