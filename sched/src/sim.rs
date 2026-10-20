//! A seeded, event-driven simulator that drives the real [`Core`] and policies the
//! way the kernel does: a 1 ms timer tick that wakes sleepers and asks the policy
//! about preemption, wake-ups that may preempt at once, and threads that compute,
//! sleep on timers or wait for outside events.
//!
//! Time jumps from event to event (a burst ending, an outside event, a tick), so a
//! simulated second costs a few thousand steps. Every random length comes from a
//! per-process stream keyed by the seed and the process's index in its mix, and
//! each process draws in its own program order, so a process does the same work
//! whatever the policy or the other processes do. Arrivals and service times use
//! separate streams, so event `k` of a probe arrives at the same moment and needs
//! the same CPU in a loaded run and in a run of the probe alone. That is what lets
//! the judge subtract the unloaded latency of the same seed.

use crate::classes::TICK_NS;
use crate::{Core, IDLE, MAX_THREADS, SchedPolicy, State, Tid};
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

pub const US: u64 = 1_000;
pub const MS: u64 = 1_000_000;
/// The kernel's watchdog limit.
pub const WATCHDOG_NS: u64 = 200 * MS;
const BOOT: Tid = 1;
const NO_PROCESS: u8 = u8::MAX;

/// An inclusive range of nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub low: u64,
    pub high: u64,
}

impl Span {
    pub const fn new(low: u64, high: u64) -> Self {
        Self { low, high }
    }

    /// A length from the span, as the simulator and the `work` program draw it.
    pub fn draw(&self, rng: &mut Rng) -> u64 {
        self.low + rng.below(self.high.saturating_sub(self.low) + 1)
    }
}

/// What a simulated process does. Each process is one thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behavior {
    /// Always runnable.
    Hog,
    /// Computes for a time from `run`, sleeps on a timer for a time from `sleep`
    /// (woken at a tick, as kernel sleeps are), and repeats.
    Wakeful { run: Span, sleep: Span },
    /// Waits for outside events, such as key presses, whose gaps come from `gap`,
    /// and handles each with a burst from `burst`; events that arrive while it is
    /// busy queue. An event's latency runs from its scheduled time to the end of
    /// its burst, so time spent queued counts.
    Probe { gap: Span, burst: Span },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcSpec {
    pub name: &'static str,
    pub behavior: Behavior,
}

/// splitmix64: small, fast and the same on every host.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A value in `0..n`, or 0 when `n` is 0.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }

    /// The stream for one purpose of one process: 1 for the CPU each burst needs,
    /// 2 for sleeps and gaps. The `work` program draws from the same streams.
    pub fn stream(seed: u64, process: usize, purpose: u64) -> Self {
        let mut mixer = Self(seed ^ ((process as u64 + 1) << 20) ^ (purpose << 52));
        Self(mixer.next_u64())
    }
}

/// One finished event of a probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventRecord {
    /// When it was scheduled to happen.
    pub at: u64,
    pub latency_ns: u64,
}

struct Thread {
    /// The process's index in its mix.
    index: usize,
    tid: Tid,
    behavior: Behavior,
    /// CPU time each burst needs.
    work: Rng,
    /// Sleep lengths and gaps between events.
    timing: Rng,
    /// CPU still needed by the current burst; `u64::MAX` for a hog.
    left: u64,
    next_event: u64,
    pending: VecDeque<u64>,
    events: Vec<EventRecord>,
}

pub struct Sim {
    core: Core,
    now: u64,
    next_tick: u64,
    threads: Vec<Thread>,
    /// Position in `threads` of each thread ID, or `NO_PROCESS`.
    slot_of: [u8; MAX_THREADS],
}

impl Sim {
    /// Every process of `procs`, under `policy`.
    pub fn new(procs: &[ProcSpec], seed: u64, policy: Box<dyn SchedPolicy>) -> Self {
        Self::build(procs, seed, policy, None)
    }

    /// Only process `only`, with the same streams it has in the full mix.
    pub fn alone(procs: &[ProcSpec], only: usize, seed: u64, policy: Box<dyn SchedPolicy>) -> Self {
        Self::build(procs, seed, policy, Some(only))
    }

    fn build(
        procs: &[ProcSpec],
        seed: u64,
        policy: Box<dyn SchedPolicy>,
        only: Option<usize>,
    ) -> Self {
        assert!(
            procs.len() < MAX_THREADS - 2,
            "too many simulated processes"
        );
        let mut core = Core::new(policy, IDLE, WATCHDOG_NS, 0);
        core.adopt_running(BOOT, "boot");
        let mut sim = Self {
            core,
            now: 0,
            next_tick: TICK_NS,
            threads: Vec::new(),
            slot_of: [NO_PROCESS; MAX_THREADS],
        };
        for (index, spec) in procs.iter().enumerate() {
            if only.is_some_and(|only| only != index) {
                continue;
            }
            let tid = sim.core.spawn(spec.name, 0).expect("a free thread slot");
            let mut work = Rng::stream(seed, index, 1);
            let mut timing = Rng::stream(seed, index, 2);
            let (left, next_event) = match spec.behavior {
                Behavior::Hog => (u64::MAX, u64::MAX),
                Behavior::Wakeful { run, .. } => (run.draw(&mut work), u64::MAX),
                // Runs once, finds nothing to do and blocks.
                Behavior::Probe { gap, .. } => (0, gap.draw(&mut timing)),
            };
            sim.slot_of[tid] = sim.threads.len() as u8;
            sim.threads.push(Thread {
                index,
                tid,
                behavior: spec.behavior,
                work,
                timing,
                left,
                next_event,
                pending: VecDeque::new(),
                events: Vec::new(),
            });
        }
        sim.core.exit_current(0);
        sim
    }

    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn core(&self) -> &Core {
        &self.core
    }

    pub fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn thread_of(&self, index: usize) -> Option<&Thread> {
        self.threads.iter().find(|thread| thread.index == index)
    }

    /// The thread ID of process `index` of the mix, if it runs here.
    pub fn tid(&self, index: usize) -> Option<Tid> {
        self.thread_of(index).map(|thread| thread.tid)
    }

    /// The finished events of process `index`, in the order they were scheduled.
    pub fn events(&self, index: usize) -> &[EventRecord] {
        self.thread_of(index).map_or(&[], |thread| &thread.events)
    }

    /// When the events of process `index` that have not finished were scheduled.
    pub fn pending(&self, index: usize) -> impl Iterator<Item = u64> + '_ {
        self.thread_of(index)
            .into_iter()
            .flat_map(|thread| thread.pending.iter().copied())
    }

    fn slot(&self, tid: Tid) -> Option<usize> {
        self.slot_of
            .get(tid)
            .filter(|&&slot| slot != NO_PROCESS)
            .map(|&slot| usize::from(slot))
    }

    /// Runs until `end`.
    pub fn run_until(&mut self, end: u64) {
        while self.now < end {
            let running = self.slot(self.core.current());
            let burst_end =
                running.map_or(u64::MAX, |i| self.now.saturating_add(self.threads[i].left));
            let event = self
                .threads
                .iter()
                .map(|t| t.next_event)
                .min()
                .unwrap_or(u64::MAX);
            let next = burst_end.min(event).min(self.next_tick).min(end);
            if let Some(i) = running
                && self.threads[i].left != u64::MAX
            {
                self.threads[i].left -= next - self.now;
            }
            self.now = next;
            if let Some(i) = running
                && self.threads[i].left == 0
            {
                self.finish_burst(i);
            }
            for i in 0..self.threads.len() {
                if self.threads[i].next_event <= self.now {
                    self.arrive(i);
                }
            }
            if self.now == self.next_tick {
                self.core.tick(self.now);
                self.next_tick += TICK_NS;
            }
        }
    }

    fn finish_burst(&mut self, i: usize) {
        let now = self.now;
        let thread = &mut self.threads[i];
        match thread.behavior {
            Behavior::Hog => {}
            Behavior::Wakeful { run, sleep } => {
                let nap = sleep.draw(&mut thread.timing);
                thread.left = run.draw(&mut thread.work);
                self.core.sleep_current(now + nap, now);
            }
            Behavior::Probe { burst, .. } => {
                if let Some(at) = thread.pending.pop_front() {
                    thread.events.push(EventRecord {
                        at,
                        latency_ns: now - at,
                    });
                }
                if thread.pending.is_empty() {
                    self.core.block_current(now);
                } else {
                    thread.left = burst.draw(&mut thread.work);
                }
            }
        }
    }

    fn arrive(&mut self, i: usize) {
        let now = self.now;
        let thread = &mut self.threads[i];
        let Behavior::Probe { gap, burst } = thread.behavior else {
            return;
        };
        // An event that finds the probe idle starts its burst now, whether the
        // probe is blocked or has not run yet, so event k always gets burst k.
        let idle = thread.pending.is_empty();
        thread.pending.push_back(thread.next_event);
        thread.next_event += gap.draw(&mut thread.timing).max(1);
        if idle {
            thread.left = burst.draw(&mut thread.work);
        }
        let tid = thread.tid;
        if self.core.state(tid) == State::Blocked && self.core.wake(tid, now) {
            self.core.preempt_if_pending(now);
        }
    }
}

#[cfg(test)]
mod tests;
