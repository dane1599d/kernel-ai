//! Two policies built on Interactive's rules for M10's policy agent.
//!
//! [`Classes`] is Interactive with per-thread labels. A thread labelled
//! [`Class::Background`] gives up only its wake's place ahead of unlabelled wakes:
//! it waits in a ring of its own, after every unlabelled thread that woke and
//! before the normal ring (preempted threads, and hogs that never sleep), and it
//! preempts on wake exactly where Interactive would. So it never loses CPU to an
//! always-runnable unlabelled thread that Interactive would have run after it (the
//! founder's rule of 2026-10-21: the builds lost half their CPU to the hogs when a
//! label put their wakes in the normal ring). Once it runs, it keeps what
//! Interactive gives any thread that woke, a 2 ms slice that a wake cannot cut
//! short. So a label can only demote: no labelled thread overtakes an unlabelled
//! one that Interactive would run first, and it preempts only where Interactive
//! would. Two bounded parameters come from a preset ([`ClassParams`]): the normal
//! ring's slice, and a maximum wait. A background thread whose runnable period
//! began with a wake, and which has waited that long, is picked before any thread
//! woken after it: exactly where Interactive would have put it, so the floor caps
//! the demotion without ever promoting.
//!
//! [`Responsive`] is the honest rule-based baseline: Interactive, except that a
//! woken thread is boosted, and may preempt, only if its last run burst was short.
//! It needs no labels and no model, so it is what the model has to beat.
//!
//! Policies cannot read the clock, so `Classes` counts ticks: the kernel and the
//! simulator call `tick` once per [`TICK_NS`] while anything runs, and nothing
//! waits while the CPU idles.

use crate::{EnqueueReason, MAX_THREADS, Ring, SchedPolicy, Tid};

pub const MS: u64 = 1_000_000;
/// The kernel's 1 kHz timer tick; the simulator ticks at the same rate.
pub const TICK_NS: u64 = MS;
/// Interactive's slices, which both policies keep for boosted threads.
pub const BOOSTED_SLICE_NS: u64 = 2 * MS;
pub const INTERACTIVE_NORMAL_SLICE_NS: u64 = 10 * MS;
/// Bounds every preset is checked against, in code.
pub const NORMAL_SLICE_BOUNDS_NS: (u64, u64) = (4 * MS, 20 * MS);
pub const MAX_WAIT_BOUNDS_NS: (u64, u64) = (10 * MS, 80 * MS);
/// `Responsive` boosts a woken thread only if its last burst was shorter.
pub const SHORT_BURST_NS: u64 = MS;

/// A thread's label. Every thread starts `Normal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Class {
    #[default]
    Normal,
    Background,
}

/// The two numbers a preset sets, only ever constructed inside their bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassParams {
    normal_slice_ns: u64,
    max_wait_ns: u64,
}

impl ClassParams {
    pub fn new(normal_slice_ns: u64, max_wait_ns: u64) -> Option<Self> {
        let within = |value: u64, (low, high): (u64, u64)| (low..=high).contains(&value);
        (within(normal_slice_ns, NORMAL_SLICE_BOUNDS_NS) && within(max_wait_ns, MAX_WAIT_BOUNDS_NS))
            .then_some(Self {
                normal_slice_ns,
                max_wait_ns,
            })
    }

    pub fn normal_slice_ns(&self) -> u64 {
        self.normal_slice_ns
    }

    pub fn max_wait_ns(&self) -> u64 {
        self.max_wait_ns
    }
}

/// Interactive with background labels and a bounded wait.
pub struct Classes {
    /// Unlabelled threads that woke, labelled ones that woke, and the rest.
    woken: Ring,
    background_woken: Ring,
    normal: Ring,
    boosted: [bool; MAX_THREADS],
    background: [bool; MAX_THREADS],
    /// The thread's runnable period began with a wake: Interactive would have
    /// boosted it.
    wake_started: [bool; MAX_THREADS],
    /// Tick count when the thread was queued, and a sequence number giving the
    /// order threads were queued in.
    queued_tick: [u64; MAX_THREADS],
    order: [u64; MAX_THREADS],
    next_order: u64,
    ticks: u64,
    params: ClassParams,
}

impl Classes {
    pub fn new(params: ClassParams) -> Self {
        Self {
            woken: Ring::default(),
            background_woken: Ring::default(),
            normal: Ring::default(),
            boosted: [false; MAX_THREADS],
            background: [false; MAX_THREADS],
            wake_started: [false; MAX_THREADS],
            queued_tick: [0; MAX_THREADS],
            order: [0; MAX_THREADS],
            next_order: 0,
            ticks: 0,
            params,
        }
    }

    pub fn params(&self) -> ClassParams {
        self.params
    }

    fn max_wait_ticks(&self) -> u64 {
        self.params.max_wait_ns.div_ceil(TICK_NS)
    }

    /// The longest-queued background thread that woke and has waited past the
    /// maximum. A thread queued in a tick is counted from the next one, so the
    /// floor never fires early; ticks the kernel defers are not counted, so it
    /// can fire late by those.
    fn oldest_due(&self) -> Option<Tid> {
        let limit = self.max_wait_ticks();
        self.background_woken
            .iter()
            .filter(|&tid| self.ticks.saturating_sub(self.queued_tick[tid]) > limit)
            .min_by_key(|&tid| self.order[tid])
    }

    /// Whether a running thread is shielded from wake preemption and runs the
    /// boosted slice: as Interactive treats any thread that woke, labelled or not.
    /// A background thread keeps this once it runs; only its place in the queue
    /// is lost.
    fn shielded(&self, tid: Tid) -> bool {
        self.wake_started.get(tid).copied().unwrap_or(false)
    }

    /// Puts `tid` into `ring` at its place in queueing order.
    fn insert_in_order(&self, ring: &Ring, tid: Tid) -> Ring {
        let mut rebuilt = Ring::default();
        let mut placed = false;
        for queued in ring.iter() {
            if !placed && self.order[tid] < self.order[queued] {
                rebuilt.push_back(tid);
                placed = true;
            }
            rebuilt.push_back(queued);
        }
        if !placed {
            rebuilt.push_back(tid);
        }
        rebuilt
    }
}

impl SchedPolicy for Classes {
    fn name(&self) -> &'static str {
        "classes"
    }

    fn enqueue(&mut self, tid: Tid, reason: EnqueueReason) {
        if tid >= MAX_THREADS {
            return;
        }
        let woke = reason == EnqueueReason::Woken;
        self.wake_started[tid] = woke;
        self.boosted[tid] = woke && !self.background[tid];
        self.queued_tick[tid] = self.ticks;
        self.order[tid] = self.next_order;
        self.next_order += 1;
        match (woke, self.background[tid]) {
            (true, false) => self.woken.push_back(tid),
            (true, true) => self.background_woken.push_back(tid),
            (false, _) => self.normal.push_back(tid),
        };
    }

    fn dequeue(&mut self, tid: Tid) {
        self.woken.remove(tid);
        self.background_woken.remove(tid);
        self.normal.remove(tid);
    }

    fn pick_next(&mut self) -> Option<Tid> {
        if let Some(due) = self.oldest_due() {
            let before_woken = self
                .woken
                .front()
                .is_none_or(|first| self.order[due] < self.order[first]);
            if before_woken {
                self.background_woken.remove(due);
                return Some(due);
            }
        }
        self.woken
            .pop_front()
            .or_else(|| self.background_woken.pop_front())
            .or_else(|| self.normal.pop_front())
    }

    fn tick(&mut self, current: Tid, slice_ns: u64) -> bool {
        self.ticks += 1;
        let shielded = self.shielded(current);
        let limit = if shielded {
            BOOSTED_SLICE_NS
        } else {
            self.params.normal_slice_ns
        };
        let queued =
            !(self.woken.is_empty() && self.background_woken.is_empty() && self.normal.is_empty());
        // A due thread would have been boosted under Interactive, so, like a wake,
        // it preempts only a thread that is not shielded.
        (slice_ns >= limit && queued) || (!shielded && self.oldest_due().is_some())
    }

    /// As Interactive: a wake preempts a thread that is not shielded, labelled
    /// or not, so a labelled build still cuts a hog's slice short.
    fn preempt_on_wake(&self, _woken: Tid, current: Tid) -> bool {
        !self.shielded(current)
    }

    fn set_class(&mut self, tid: Tid, class: Class) {
        let Some(label) = self.background.get_mut(tid) else {
            return;
        };
        *label = class == Class::Background;
        let woke = self.wake_started[tid];
        if *label && self.boosted[tid] {
            // A thread that loses its boost moves behind the unlabelled wakes, in
            // order.
            self.boosted[tid] = false;
            if self.woken.remove(tid) {
                self.background_woken = self.insert_in_order(&self.background_woken, tid);
            }
        } else if !*label && woke && !self.boosted[tid] {
            // Unlabelled, a thread that woke gets back what Interactive gives it.
            self.boosted[tid] = true;
            if self.background_woken.remove(tid) {
                self.woken = self.insert_in_order(&self.woken, tid);
            }
        }
    }

    fn reset(&mut self) {
        self.woken.clear();
        self.background_woken.clear();
        self.normal.clear();
        self.boosted = [false; MAX_THREADS];
        self.wake_started = [false; MAX_THREADS];
    }
}

/// Interactive, but only threads with short bursts are boosted and preempt. A
/// burst is what the ticks saw of it: a run that ends between ticks counts only
/// up to the last tick, so bursts up to about 2 ms can pass as short. That is the
/// rule's honest resolution with a 1 kHz tick and no clock.
pub struct Responsive {
    woken: Ring,
    normal: Ring,
    boosted: [bool; MAX_THREADS],
    /// Slice length the latest tick saw for each thread's current run, and the
    /// burst so far across preemptions.
    last_slice: [u64; MAX_THREADS],
    burst: [u64; MAX_THREADS],
}

impl Default for Responsive {
    fn default() -> Self {
        Self {
            woken: Ring::default(),
            normal: Ring::default(),
            boosted: [false; MAX_THREADS],
            last_slice: [0; MAX_THREADS],
            burst: [0; MAX_THREADS],
        }
    }
}

impl Responsive {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SchedPolicy for Responsive {
    fn name(&self) -> &'static str {
        "responsive"
    }

    fn enqueue(&mut self, tid: Tid, reason: EnqueueReason) {
        if tid >= MAX_THREADS {
            return;
        }
        let ran = core::mem::take(&mut self.last_slice[tid]);
        self.boosted[tid] = match reason {
            EnqueueReason::Woken => core::mem::take(&mut self.burst[tid]) + ran < SHORT_BURST_NS,
            EnqueueReason::Preempted | EnqueueReason::Yielded => {
                self.burst[tid] += ran;
                false
            }
            EnqueueReason::New => {
                self.burst[tid] = 0;
                false
            }
        };
        if self.boosted[tid] {
            self.woken.push_back(tid);
        } else {
            self.normal.push_back(tid);
        }
    }

    fn dequeue(&mut self, tid: Tid) {
        self.woken.remove(tid);
        self.normal.remove(tid);
        if let (Some(seen), Some(burst)) = (self.last_slice.get_mut(tid), self.burst.get_mut(tid)) {
            (*seen, *burst) = (0, 0);
        }
    }

    fn pick_next(&mut self) -> Option<Tid> {
        self.woken.pop_front().or_else(|| self.normal.pop_front())
    }

    fn tick(&mut self, current: Tid, slice_ns: u64) -> bool {
        let boosted = self.boosted.get(current).copied().unwrap_or(false);
        if let Some(seen) = self.last_slice.get_mut(current) {
            *seen = slice_ns;
        }
        let limit = if boosted {
            BOOSTED_SLICE_NS
        } else {
            INTERACTIVE_NORMAL_SLICE_NS
        };
        slice_ns >= limit && !(self.woken.is_empty() && self.normal.is_empty())
    }

    fn preempt_on_wake(&self, woken: Tid, current: Tid) -> bool {
        let get = |tid: Tid| self.boosted.get(tid).copied().unwrap_or(false);
        get(woken) && !get(current)
    }

    fn reset(&mut self) {
        self.woken.clear();
        self.normal.clear();
        self.boosted = [false; MAX_THREADS];
        self.last_slice = [0; MAX_THREADS];
        self.burst = [0; MAX_THREADS];
    }
}

#[cfg(test)]
mod tests;
