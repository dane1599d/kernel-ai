//! Per-thread accounting kept by [`Core`](crate::Core): CPU time, wakes,
//! preemptions, the longest runnable wait and wake-to-run latency.
//!
//! Latency is kept two ways. The last [`SAMPLES`] wake-to-run times of each thread
//! are kept exactly in a ring, so a judge can take a true percentile over a window
//! (a ratio such as 0.8x cannot be read off power-of-two buckets). A log2 histogram
//! counts every sample, for display. Counters only grow, so two [`Snapshot`]s give
//! a window by subtraction; the longest wait is not subtractable, so it also has a
//! window maximum that [`Core::begin_window`](crate::Core::begin_window) restarts.
//!
//! Everything is sized at construction: recording never allocates, so the kernel
//! can record from interrupts. A Core's counters cover the threads while it owns
//! them. Cores combine with [`Snapshot::absorb`]: counters add and maxima take the
//! larger, but sample sequence numbers belong to one Core's ring, so exact samples
//! are read Core by Core. A thread that migrates through
//! [`Core::detach_waiting`](crate::Core::detach_waiting) keeps its wait and its
//! wake. A thread slot that is reused keeps counting, so windows stay correct, and
//! a window spanning the reuse mixes the two threads.
//!
//! Snapshots and windows are about 12 KiB each: kernel code fills buffers it owns
//! with [`Core::snapshot_into`](crate::Core::snapshot_into) and
//! [`Snapshot::window_into`] instead of returning them on its stack.

use crate::{EnqueueReason, MAX_THREADS, Tid};
use alloc::boxed::Box;
use alloc::vec::Vec;

/// Exact wake-to-run samples kept per thread.
pub const SAMPLES: usize = 256;
/// Log2 latency buckets: bucket `i` counts samples in [2^i, 2^(i+1)) ns, and bucket
/// 0 also counts 0. The last bucket holds everything from about 2.1 s up.
pub const BUCKETS: usize = 32;

/// One thread's counters, all monotonic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counters {
    /// Time on the CPU, including the run in progress at the snapshot.
    pub cpu_ns: u64,
    pub wakes: u64,
    /// Times the thread was taken off the CPU while it could still run.
    pub preemptions: u64,
    /// Wake-to-run samples recorded so far; also the sequence number of the next.
    pub samples: u64,
    pub latency_sum_ns: u64,
    pub hist: [u32; BUCKETS],
    /// Longest runnable wait that ended, or was still in progress, since the last
    /// window began; a wait counts from when it began, even before the window.
    pub window_max_wait_ns: u64,
    /// Longest wake-to-run latency since the last window began.
    pub window_max_latency_ns: u64,
}

impl Counters {
    const ZERO: Self = Self {
        cpu_ns: 0,
        wakes: 0,
        preemptions: 0,
        samples: 0,
        latency_sum_ns: 0,
        hist: [0; BUCKETS],
        window_max_wait_ns: 0,
        window_max_latency_ns: 0,
    };
}

struct Slot {
    counters: Counters,
    /// The thread's current runnable period began with a wake.
    woken: bool,
    /// Wait already spent on another Core before a migration.
    carried_ns: u64,
    /// Wake-to-run times in nanoseconds. A wait past `u32::MAX` (4.29 s) is stored
    /// as that: the 200 ms watchdog ejects a policy long before.
    ring: [u32; SAMPLES],
}

/// The accounting tables of one Core.
pub struct Stats {
    slots: Box<[Slot]>,
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

impl Stats {
    /// Allocates the tables: about 80 KiB, once, when the Core is built.
    pub fn new() -> Self {
        let slots: Vec<Slot> = (0..MAX_THREADS)
            .map(|_| Slot {
                counters: Counters::ZERO,
                woken: false,
                carried_ns: 0,
                ring: [0; SAMPLES],
            })
            .collect();
        Self {
            slots: slots.into_boxed_slice(),
        }
    }

    /// The thread became runnable for `reason`.
    pub(crate) fn runnable(&mut self, tid: Tid, reason: EnqueueReason) {
        let Some(slot) = self.slots.get_mut(tid) else {
            return;
        };
        slot.woken = reason == EnqueueReason::Woken;
        slot.carried_ns = 0;
        if slot.woken {
            slot.counters.wakes += 1;
        }
    }

    /// The running thread was put back in the queue while still runnable.
    pub(crate) fn preempted(&mut self, tid: Tid) {
        if let Some(slot) = self.slots.get_mut(tid) {
            slot.counters.preemptions += 1;
        }
    }

    /// `tid` ran for `ns` and is leaving the CPU (or being picked again).
    pub(crate) fn ran(&mut self, tid: Tid, ns: u64) {
        if let Some(slot) = self.slots.get_mut(tid) {
            slot.counters.cpu_ns += ns;
        }
    }

    /// `tid` was picked after waiting `waited_ns` as runnable here.
    pub(crate) fn picked(&mut self, tid: Tid, waited_ns: u64) {
        let Some(slot) = self.slots.get_mut(tid) else {
            return;
        };
        let waited_ns = waited_ns + core::mem::take(&mut slot.carried_ns);
        let c = &mut slot.counters;
        c.window_max_wait_ns = c.window_max_wait_ns.max(waited_ns);
        if !core::mem::take(&mut slot.woken) {
            return;
        }
        slot.ring[(c.samples % SAMPLES as u64) as usize] = waited_ns.min(u32::MAX.into()) as u32;
        c.samples += 1;
        c.latency_sum_ns += waited_ns;
        c.hist[bucket(waited_ns)] = c.hist[bucket(waited_ns)].wrapping_add(1);
        c.window_max_latency_ns = c.window_max_latency_ns.max(waited_ns);
    }

    /// A runnable thread leaves for another Core after waiting `waited_ns`: the
    /// wait counts here, and whether it woke goes with it.
    pub(crate) fn left(&mut self, tid: Tid, waited_ns: u64) -> bool {
        let Some(slot) = self.slots.get_mut(tid) else {
            return false;
        };
        let c = &mut slot.counters;
        c.window_max_wait_ns = c.window_max_wait_ns.max(waited_ns + slot.carried_ns);
        slot.carried_ns = 0;
        core::mem::take(&mut slot.woken)
    }

    /// A migrated thread arrived after waiting `waited_ns` elsewhere.
    pub(crate) fn arrived(&mut self, tid: Tid, woken: bool, waited_ns: u64) {
        if let Some(slot) = self.slots.get_mut(tid) {
            slot.woken = woken;
            slot.carried_ns = waited_ns;
        }
    }

    pub(crate) fn begin_window(&mut self) {
        for slot in self.slots.iter_mut() {
            slot.counters.window_max_wait_ns = 0;
            slot.counters.window_max_latency_ns = 0;
        }
    }

    pub fn counters(&self, tid: Tid) -> Counters {
        self.slots
            .get(tid)
            .map_or(Counters::ZERO, |slot| slot.counters)
    }

    /// The samples of `tid` with sequence numbers in `from..to` (from
    /// [`Counters::samples`] of two snapshots), oldest first, into `out`. Returns
    /// how many were written, or `None` if some have been overwritten since or
    /// `out` is too small.
    pub fn samples(&self, tid: Tid, from: u64, to: u64, out: &mut [u32]) -> Option<usize> {
        let slot = self.slots.get(tid)?;
        let count = usize::try_from(to.checked_sub(from)?).ok()?;
        if to > slot.counters.samples
            || slot.counters.samples - from > SAMPLES as u64
            || count > out.len()
        {
            return None;
        }
        for (i, value) in out[..count].iter_mut().enumerate() {
            *value = slot.ring[((from + i as u64) % SAMPLES as u64) as usize];
        }
        Some(count)
    }
}

/// The log2 bucket of a latency.
pub fn bucket(ns: u64) -> usize {
    match ns {
        0 => 0,
        _ => (63 - ns.leading_zeros() as usize).min(BUCKETS - 1),
    }
}

/// Every thread's counters at one moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub at: u64,
    pub threads: [Counters; MAX_THREADS],
}

/// What each thread did between two snapshots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreadWindow {
    pub cpu_ns: u64,
    pub wakes: u64,
    pub preemptions: u64,
    /// Sequence numbers of the window's wake-to-run samples, for
    /// [`Stats::samples`].
    pub first_sample: u64,
    pub end_sample: u64,
    pub max_wait_ns: u64,
    pub max_latency_ns: u64,
    pub hist: [u32; BUCKETS],
}

impl ThreadWindow {
    pub fn sample_count(&self) -> u64 {
        self.end_sample - self.first_sample
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub ns: u64,
    pub threads: [ThreadWindow; MAX_THREADS],
}

impl Window {
    pub const EMPTY: Window = Window {
        ns: 0,
        threads: [ThreadWindow {
            cpu_ns: 0,
            wakes: 0,
            preemptions: 0,
            first_sample: 0,
            end_sample: 0,
            max_wait_ns: 0,
            max_latency_ns: 0,
            hist: [0; BUCKETS],
        }; MAX_THREADS],
    };
}

impl Snapshot {
    pub const EMPTY: Snapshot = Snapshot {
        at: 0,
        threads: [Counters::ZERO; MAX_THREADS],
    };

    /// Adds another Core's snapshot taken at the same moment: counters add,
    /// maxima take the larger. Sample sequence numbers stop meaning anything.
    pub fn absorb(&mut self, other: &Snapshot) {
        for (mine, theirs) in self.threads.iter_mut().zip(&other.threads) {
            mine.cpu_ns += theirs.cpu_ns;
            mine.wakes += theirs.wakes;
            mine.preemptions += theirs.preemptions;
            mine.samples += theirs.samples;
            mine.latency_sum_ns += theirs.latency_sum_ns;
            for (bucket, count) in mine.hist.iter_mut().zip(theirs.hist) {
                *bucket = bucket.wrapping_add(count);
            }
            mine.window_max_wait_ns = mine.window_max_wait_ns.max(theirs.window_max_wait_ns);
            mine.window_max_latency_ns =
                mine.window_max_latency_ns.max(theirs.window_max_latency_ns);
        }
    }

    /// The window from `earlier` to this snapshot, into `out`. Maxima are this
    /// snapshot's, that is since the last `begin_window`, which should be when
    /// `earlier` was taken.
    pub fn window_into(&self, earlier: &Snapshot, out: &mut Window) {
        out.ns = self.at.saturating_sub(earlier.at);
        for tid in 0..MAX_THREADS {
            let (now, then) = (&self.threads[tid], &earlier.threads[tid]);
            let window = &mut out.threads[tid];
            window.cpu_ns = now.cpu_ns.saturating_sub(then.cpu_ns);
            window.wakes = now.wakes.saturating_sub(then.wakes);
            window.preemptions = now.preemptions.saturating_sub(then.preemptions);
            window.first_sample = then.samples;
            window.end_sample = now.samples.max(then.samples);
            window.max_wait_ns = now.window_max_wait_ns;
            window.max_latency_ns = now.window_max_latency_ns;
            for b in 0..BUCKETS {
                window.hist[b] = now.hist[b].wrapping_sub(then.hist[b]);
            }
        }
    }

    /// [`Snapshot::window_into`] returning the window: for host code.
    pub fn since(&self, earlier: &Snapshot) -> Window {
        let mut out = Window::EMPTY;
        self.window_into(earlier, &mut out);
        out
    }
}

/// The nearest-rank percentile `pct` (1 to 100) of `sorted`, or 0 when empty.
pub fn percentile(sorted: &[u64], pct: u64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let n = sorted.len() as u64;
    let rank = (pct.clamp(1, 100) * n).div_ceil(100).max(1);
    sorted[(rank - 1) as usize]
}

#[cfg(test)]
mod tests;
