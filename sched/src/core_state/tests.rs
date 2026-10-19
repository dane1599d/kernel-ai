//! A discrete-time simulator drives `Core` the way the kernel does: a 1 ms timer
//! tick, external wake-ups standing in for key presses, and threads that either
//! compute forever or run briefly and block.
use super::*;
use crate::{IDLE, Interactive, RoundRobin};
use alloc::vec::Vec;

const US: u64 = 1_000;
const MS: u64 = 1_000_000;
const STEP: u64 = 100 * US;
const WATCHDOG: u64 = 200 * MS;

fn core_with_boot(policy: Box<dyn SchedPolicy>) -> Core {
    let mut core = Core::new(policy, IDLE, WATCHDOG, 0);
    core.adopt_running(1, "boot");
    core
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Hog,
    /// Woken by an external event every `period`, runs `burst`, then blocks.
    Reactive {
        period: u64,
        burst: u64,
    },
}

struct Sim {
    core: Core,
    now: u64,
    kinds: [Option<Kind>; MAX_THREADS],
    cpu: [u64; MAX_THREADS],
    burst_left: [u64; MAX_THREADS],
    next_event: [u64; MAX_THREADS],
    woke_at: [Option<u64>; MAX_THREADS],
    latencies: Vec<u64>,
    boot_exited: bool,
}

impl Sim {
    fn new(policy: Box<dyn SchedPolicy>) -> Self {
        Self {
            core: core_with_boot(policy),
            now: 0,
            kinds: [None; MAX_THREADS],
            cpu: [0; MAX_THREADS],
            burst_left: [0; MAX_THREADS],
            next_event: [u64::MAX; MAX_THREADS],
            woke_at: [None; MAX_THREADS],
            latencies: Vec::new(),
            boot_exited: false,
        }
    }

    fn spawn(&mut self, kind: Kind) -> Tid {
        let tid = self.core.spawn("sim", self.now).unwrap();
        self.kinds[tid] = Some(kind);
        if let Kind::Reactive { period, burst } = kind {
            self.burst_left[tid] = burst;
            self.next_event[tid] = self.now + period;
        }
        tid
    }

    fn switched_to(&mut self, tid: Tid) {
        if let Some(woke) = self.woke_at[tid].take() {
            self.latencies.push(self.now - woke);
        }
    }

    fn run(&mut self, duration: u64) {
        if !self.boot_exited {
            self.boot_exited = true;
            let next = self.core.exit_current(self.now);
            self.switched_to(next);
        }
        let end = self.now + duration;
        while self.now < end {
            self.now += STEP;
            let current = self.core.current();
            self.cpu[current] += STEP;
            if let Some(Kind::Reactive { burst, .. }) = self.kinds[current] {
                self.burst_left[current] = self.burst_left[current].saturating_sub(STEP);
                if self.burst_left[current] == 0 {
                    self.burst_left[current] = burst;
                    let next = self.core.block_current(self.now);
                    self.switched_to(next);
                }
            }
            for tid in 0..MAX_THREADS {
                if let Some(Kind::Reactive { period, .. }) = self.kinds[tid]
                    && self.next_event[tid] <= self.now
                {
                    self.next_event[tid] += period;
                    if self.core.state(tid) == State::Blocked {
                        self.woke_at[tid] = Some(self.now);
                    }
                    if self.core.wake(tid, self.now)
                        && let Some(next) = self.core.preempt(self.now)
                    {
                        self.switched_to(next);
                    }
                }
            }
            if self.now.is_multiple_of(MS)
                && let Some(next) = self.core.tick(self.now)
            {
                self.switched_to(next);
            }
        }
    }

    fn max_latency(&self) -> u64 {
        self.latencies.iter().copied().max().unwrap_or(0)
    }
}

fn hogs_and_one_reactive(policy: Box<dyn SchedPolicy>) -> (Sim, Vec<Tid>) {
    let mut sim = Sim::new(policy);
    let hogs: Vec<Tid> = (0..8).map(|_| sim.spawn(Kind::Hog)).collect();
    sim.spawn(Kind::Reactive {
        period: 50 * MS,
        burst: 300 * US,
    });
    sim.run(2_000 * MS);
    (sim, hogs)
}

#[test]
fn interactive_policy_keeps_wake_latency_under_20ms_with_8_hogs() {
    let (sim, hogs) = hogs_and_one_reactive(Box::new(Interactive::new()));
    assert!(sim.latencies.len() >= 35, "too few wake-ups measured");
    assert!(
        sim.max_latency() < 20 * MS,
        "max latency {} ns",
        sim.max_latency()
    );
    let shares: Vec<u64> = hogs.iter().map(|&t| sim.cpu[t]).collect();
    let (min, max) = (shares.iter().min().unwrap(), shares.iter().max().unwrap());
    assert!(*min * 2 >= *max, "hogs were treated unfairly: {shares:?}");
    assert_eq!(sim.core.ejections(), 0);
}

#[test]
fn round_robin_latency_grows_with_the_number_of_hogs() {
    let (sim, _) = hogs_and_one_reactive(Box::new(RoundRobin::new()));
    // Eight hogs with 10 ms slices: a woken thread can wait about 80 ms.
    assert!(sim.max_latency() > 20 * MS);
    assert!(sim.max_latency() < 100 * MS);
}

/// Never schedules thread 2: the watchdog must eject it.
struct Starver(RoundRobin);

impl SchedPolicy for Starver {
    fn name(&self) -> &'static str {
        "starver"
    }
    fn enqueue(&mut self, tid: Tid, reason: EnqueueReason) {
        if tid != 2 {
            self.0.enqueue(tid, reason);
        }
    }
    fn dequeue(&mut self, tid: Tid) {
        self.0.dequeue(tid);
    }
    fn pick_next(&mut self) -> Option<Tid> {
        self.0.pick_next()
    }
    fn tick(&mut self, current: Tid, slice_ns: u64) -> bool {
        self.0.tick(current, slice_ns)
    }
    fn reset(&mut self) {
        self.0.reset();
    }
}

#[test]
fn watchdog_ejects_a_policy_that_starves_a_thread() {
    let mut sim = Sim::new(Box::new(Starver(RoundRobin::new())));
    let hogs: Vec<Tid> = (0..3).map(|_| sim.spawn(Kind::Hog)).collect();
    sim.run(1_000 * MS);
    assert_eq!(sim.core.ejections(), 1);
    assert!(sim.core.on_fallback());
    assert_eq!(sim.core.policy_name(), "round-robin");
    assert_eq!(sim.core.installed_policy_name(), "starver");
    assert!(sim.core.take_ejection().unwrap().contains("watchdog"));
    assert!(
        sim.cpu[hogs[1]] >= 10 * MS,
        "thread 2 got {} ns",
        sim.cpu[hogs[1]]
    );
}

/// Returns a thread ID that does not exist.
struct Liar;

impl SchedPolicy for Liar {
    fn name(&self) -> &'static str {
        "liar"
    }
    fn enqueue(&mut self, _: Tid, _: EnqueueReason) {}
    fn dequeue(&mut self, _: Tid) {}
    fn pick_next(&mut self) -> Option<Tid> {
        Some(MAX_THREADS - 1)
    }
    fn tick(&mut self, _: Tid, _: u64) -> bool {
        true
    }
    fn reset(&mut self) {}
}

#[test]
fn invalid_pick_is_ejected_immediately_and_work_continues() {
    let mut sim = Sim::new(Box::new(Liar));
    let hog = sim.spawn(Kind::Hog);
    sim.run(50 * MS);
    assert_eq!(sim.core.ejections(), 1);
    assert!(sim.core.take_ejection().unwrap().contains("not runnable"));
    assert!(sim.cpu[hog] > 40 * MS);
}

#[test]
fn set_policy_hands_over_runnable_threads() {
    let mut sim = Sim::new(Box::new(RoundRobin::new()));
    let hogs: Vec<Tid> = (0..4).map(|_| sim.spawn(Kind::Hog)).collect();
    sim.run(30 * MS);
    let old = sim.core.set_policy(Box::new(Interactive::new()));
    assert_eq!(old.name(), "round-robin");
    assert_eq!(sim.core.policy_name(), "interactive");
    sim.run(200 * MS);
    assert!(hogs.iter().all(|&t| sim.cpu[t] > 30 * MS));
    assert_eq!(sim.core.ejections(), 0);
}

#[test]
fn sleeping_threads_wake_on_time_and_idle_runs_otherwise() {
    let mut core = core_with_boot(Box::new(Interactive::new()));
    assert_eq!(core.sleep_current(5 * MS, 0), IDLE);
    assert_eq!(core.tick(4 * MS), None);
    assert_eq!(core.tick(5 * MS), Some(1));
    assert_eq!(core.state(1), State::Running);
    assert_eq!(core.exit_current(6 * MS), IDLE);
    assert_eq!(core.state(1), State::Dead);
    assert!(core.release(1));
    assert_eq!(core.state(1), State::Free);
    assert!(!core.release(1));
}

#[test]
fn threads_migrate_between_cores_and_load_follows() {
    // Two CPUs with idle threads 0 and 1; threads 5..9 start on the first.
    let mut a = Core::new(Box::new(Interactive::new()), 0, WATCHDOG, 0);
    let mut b = Core::new(Box::new(Interactive::new()), 1, WATCHDOG, 0);
    for tid in 5..9 {
        a.adopt(tid, "worker", EnqueueReason::New, 0);
    }
    assert_eq!(a.preempt(MS), Some(5));
    assert_eq!((a.load(), b.load()), (4, 0));
    // The running thread is never handed out, and `skip` is honoured.
    let (moved, name) = a.detach_runnable(|tid| tid == 6).unwrap();
    assert_eq!((moved, name), (7, "worker"));
    b.adopt(moved, name, EnqueueReason::Preempted, 2 * MS);
    assert_eq!((a.load(), b.load()), (3, 1));
    assert_eq!(b.tick(3 * MS), Some(7));
    assert_eq!(b.current(), 7);
    // The migrated thread is gone from the first CPU's queue.
    let mut picked = Vec::new();
    while let Some(next) = a.preempt(10 * MS) {
        if picked.contains(&next) {
            break;
        }
        picked.push(next);
    }
    assert!(!picked.contains(&7));
}

#[test]
fn remote_wake_reports_preemption_without_switching() {
    let mut core = Core::new(Box::new(Interactive::new()), 0, WATCHDOG, 0);
    core.adopt(3, "hog", EnqueueReason::New, 0);
    core.adopt(4, "typist", EnqueueReason::New, 0);
    assert_eq!(core.preempt(0), Some(3));
    assert_eq!(core.preempt(MS), Some(4));
    assert_eq!(core.block_current(MS), 3);
    assert!(core.wake(4, 2 * MS));
    assert_eq!(core.current(), 3, "wake must not switch by itself");
    assert_eq!(core.preempt_if_pending(2 * MS), Some(4));
}

#[test]
fn late_reschedule_request_does_not_demote_the_woken_thread() {
    let mut core = Core::new(Box::new(Interactive::new()), 0, WATCHDOG, 0);
    core.adopt(3, "hog", EnqueueReason::New, 0);
    core.adopt(4, "typist", EnqueueReason::New, 0);
    assert_eq!(core.preempt(0), Some(3));
    assert_eq!(core.preempt(MS), Some(4));
    assert_eq!(core.block_current(MS), 3);
    assert!(core.wake(4, 2 * MS));
    // The owner's timer tick gets there first and runs the woken thread...
    assert_eq!(core.tick(2 * MS), Some(4));
    // ...so the reschedule interrupt that arrives afterwards must do nothing.
    assert_eq!(core.preempt_if_pending(2 * MS + 1), None);
    assert_eq!(core.current(), 4);
}

#[test]
fn idle_cpu_picks_up_adopted_work_on_request() {
    let mut core = Core::new(Box::new(Interactive::new()), 0, WATCHDOG, 0);
    assert_eq!(core.preempt_if_pending(0), None);
    core.adopt(5, "new", EnqueueReason::New, 0);
    assert_eq!(core.preempt_if_pending(0), Some(5));
}
