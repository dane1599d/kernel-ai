//! The scheduling mechanism: thread states, sleep and wake, and the watchdog.
//! It never allocates after construction, so the kernel can call it from interrupts.

use crate::{EnqueueReason, IDLE, MAX_THREADS, RoundRobin, SchedPolicy, Tid};
use alloc::boxed::Box;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Free,
    /// Only the idle thread is ever in this state.
    Idle,
    Runnable {
        since: u64,
    },
    Running,
    Sleeping {
        until: u64,
    },
    Blocked,
    Dead,
}

pub struct Core {
    states: [State; MAX_THREADS],
    names: [&'static str; MAX_THREADS],
    current: Tid,
    policy: Box<dyn SchedPolicy>,
    fallback: RoundRobin,
    on_fallback: bool,
    slice_start: u64,
    watchdog_ns: u64,
    ejections: u32,
    pending_ejection: Option<&'static str>,
    switches: u64,
}

impl Core {
    /// Thread 0 is idle; thread 1 is the caller, already running.
    pub fn new(
        policy: Box<dyn SchedPolicy>,
        boot_name: &'static str,
        watchdog_ns: u64,
        now: u64,
    ) -> Self {
        let mut states = [State::Free; MAX_THREADS];
        let mut names = [""; MAX_THREADS];
        states[IDLE] = State::Idle;
        names[IDLE] = "idle";
        states[1] = State::Running;
        names[1] = boot_name;
        Self {
            states,
            names,
            current: 1,
            policy,
            fallback: RoundRobin::new(),
            on_fallback: false,
            slice_start: now,
            watchdog_ns,
            ejections: 0,
            pending_ejection: None,
            switches: 0,
        }
    }

    pub fn current(&self) -> Tid {
        self.current
    }

    pub fn state(&self, tid: Tid) -> State {
        self.states.get(tid).copied().unwrap_or(State::Free)
    }

    pub fn name(&self, tid: Tid) -> &'static str {
        self.names.get(tid).copied().unwrap_or("")
    }

    pub fn policy_name(&self) -> &'static str {
        if self.on_fallback {
            self.fallback.name()
        } else {
            self.policy.name()
        }
    }

    pub fn on_fallback(&self) -> bool {
        self.on_fallback
    }

    pub fn ejections(&self) -> u32 {
        self.ejections
    }

    pub fn switches(&self) -> u64 {
        self.switches
    }

    /// The reason for the latest ejection, once, so the caller can log it.
    pub fn take_ejection(&mut self) -> Option<&'static str> {
        self.pending_ejection.take()
    }

    /// Registers a new runnable thread. The caller must have its stack ready.
    pub fn spawn(&mut self, name: &'static str, now: u64) -> Option<Tid> {
        let tid = (IDLE + 1..MAX_THREADS).find(|&t| self.states[t] == State::Free)?;
        self.names[tid] = name;
        self.make_runnable(tid, EnqueueReason::New, now);
        Some(tid)
    }

    /// Timer tick: wakes sleepers, runs the watchdog, asks the policy about preemption.
    /// Returns the thread to switch to, if the current one should change.
    pub fn tick(&mut self, now: u64) -> Option<Tid> {
        let mut reschedule = false;
        for tid in 0..MAX_THREADS {
            if let State::Sleeping { until } = self.states[tid]
                && until <= now
            {
                self.make_runnable(tid, EnqueueReason::Woken, now);
                reschedule |= self.should_preempt_for(tid);
            }
        }
        if !self.on_fallback && self.oldest_runnable_wait(now) > self.watchdog_ns {
            self.eject("a runnable thread waited longer than the watchdog limit");
            reschedule = true;
        }
        if self.current == IDLE {
            reschedule |= self.any_runnable();
        } else {
            let slice = now.saturating_sub(self.slice_start);
            let current = self.current;
            reschedule |= self.active().tick(current, slice);
        }
        if reschedule {
            self.reschedule(now)
        } else {
            None
        }
    }

    /// Wakes a blocked or sleeping thread. Returns the thread to switch to if the
    /// wake-up should preempt the current one.
    pub fn wake(&mut self, tid: Tid, now: u64) -> Option<Tid> {
        match self.state(tid) {
            State::Blocked | State::Sleeping { .. } => {}
            _ => return None,
        }
        self.make_runnable(tid, EnqueueReason::Woken, now);
        if self.should_preempt_for(tid) {
            self.reschedule(now)
        } else {
            None
        }
    }

    /// The current thread waits for a wake-up. Returns the thread to run instead.
    pub fn block_current(&mut self, now: u64) -> Tid {
        self.leave_current(State::Blocked, now)
    }

    pub fn sleep_current(&mut self, until: u64, now: u64) -> Tid {
        self.leave_current(State::Sleeping { until }, now)
    }

    pub fn exit_current(&mut self, now: u64) -> Tid {
        self.leave_current(State::Dead, now)
    }

    /// Puts the current thread back in the queue and picks again (possibly itself).
    pub fn yield_current(&mut self, now: u64) -> Tid {
        if self.current != IDLE {
            let current = self.current;
            self.make_runnable(current, EnqueueReason::Yielded, now);
        }
        self.pick(now)
    }

    /// Installs a new policy, hands it every runnable thread, and returns the old one
    /// so the caller can drop it outside interrupt-disabled code.
    pub fn set_policy(&mut self, policy: Box<dyn SchedPolicy>) -> Box<dyn SchedPolicy> {
        let old = core::mem::replace(&mut self.policy, policy);
        self.on_fallback = false;
        self.fallback.reset();
        self.requeue_runnable_into_active();
        old
    }

    fn leave_current(&mut self, state: State, now: u64) -> Tid {
        if self.current != IDLE {
            self.states[self.current] = state;
        }
        self.pick(now)
    }

    fn should_preempt_for(&self, woken: Tid) -> bool {
        self.current == IDLE
            || if self.on_fallback {
                self.fallback.preempt_on_wake(woken, self.current)
            } else {
                self.policy.preempt_on_wake(woken, self.current)
            }
    }

    fn reschedule(&mut self, now: u64) -> Option<Tid> {
        let previous = self.current;
        if previous != IDLE && self.states[previous] == State::Running {
            self.make_runnable(previous, EnqueueReason::Preempted, now);
        }
        let next = self.pick(now);
        (next != previous).then_some(next)
    }

    fn pick(&mut self, now: u64) -> Tid {
        let next = loop {
            match self.active().pick_next() {
                Some(tid) if self.is_runnable(tid) => break tid,
                Some(_) if self.on_fallback => continue,
                Some(_) => self.eject("the policy picked a thread that is not runnable"),
                None if !self.on_fallback && self.any_runnable() => {
                    self.eject("the policy picked nothing while threads were runnable");
                }
                None => break IDLE,
            }
        };
        if next != IDLE {
            self.states[next] = State::Running;
        }
        if next != self.current {
            self.switches += 1;
        }
        self.current = next;
        self.slice_start = now;
        next
    }

    fn eject(&mut self, reason: &'static str) {
        self.on_fallback = true;
        self.ejections += 1;
        self.pending_ejection = Some(reason);
        self.fallback.reset();
        self.requeue_runnable_into_active();
    }

    /// Re-queues runnable threads into the active policy, longest-waiting first.
    fn requeue_runnable_into_active(&mut self) {
        let mut waiting = [(u64::MAX, 0); MAX_THREADS];
        let mut count = 0;
        for (tid, state) in self.states.iter().enumerate() {
            if let State::Runnable { since } = state {
                waiting[count] = (*since, tid);
                count += 1;
            }
        }
        waiting[..count].sort_unstable();
        for &(_, tid) in &waiting[..count] {
            self.active().enqueue(tid, EnqueueReason::Preempted);
        }
    }

    fn make_runnable(&mut self, tid: Tid, reason: EnqueueReason, now: u64) {
        self.states[tid] = State::Runnable { since: now };
        self.active().enqueue(tid, reason);
    }

    fn is_runnable(&self, tid: Tid) -> bool {
        tid != IDLE && matches!(self.state(tid), State::Runnable { .. })
    }

    fn any_runnable(&self) -> bool {
        self.states
            .iter()
            .any(|s| matches!(s, State::Runnable { .. }))
    }

    fn oldest_runnable_wait(&self, now: u64) -> u64 {
        self.states
            .iter()
            .filter_map(|s| match s {
                State::Runnable { since } => Some(now.saturating_sub(*since)),
                _ => None,
            })
            .max()
            .unwrap_or(0)
    }

    fn active(&mut self) -> &mut dyn SchedPolicy {
        if self.on_fallback {
            &mut self.fallback
        } else {
            &mut *self.policy
        }
    }
}

#[cfg(test)]
mod tests {
    //! A discrete-time simulator drives `Core` the way the kernel does: a 1 ms timer
    //! tick, external wake-ups standing in for key presses, and threads that either
    //! compute forever or run briefly and block.
    use super::*;
    use crate::{Interactive, RoundRobin};
    use alloc::vec::Vec;

    const US: u64 = 1_000;
    const MS: u64 = 1_000_000;
    const STEP: u64 = 100 * US;
    const WATCHDOG: u64 = 200 * MS;

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
                core: Core::new(policy, "boot", WATCHDOG, 0),
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
            // The boot thread has nothing to do in the simulation.
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
                        if let Some(next) = self.core.wake(tid, self.now) {
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
        assert!(sim.core.take_ejection().unwrap().contains("watchdog"));
        // The starved thread ran after the ejection, at least a full slice.
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
        let mut core = Core::new(Box::new(Interactive::new()), "boot", WATCHDOG, 0);
        assert_eq!(core.sleep_current(5 * MS, 0), IDLE);
        assert_eq!(core.tick(4 * MS), None);
        assert_eq!(core.tick(5 * MS), Some(1));
        assert_eq!(core.state(1), State::Running);
        assert_eq!(core.exit_current(6 * MS), IDLE);
        assert_eq!(core.state(1), State::Dead);
    }
}
