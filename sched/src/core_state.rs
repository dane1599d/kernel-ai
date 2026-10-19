//! The scheduling mechanism for one CPU: thread states, sleep and wake, migration
//! and the watchdog. It never allocates after construction, so the kernel can call
//! it from interrupts. Thread IDs are global; each CPU's `Core` tracks the threads
//! it currently owns.

use crate::{EnqueueReason, MAX_THREADS, RoundRobin, SchedPolicy, Tid};
use alloc::boxed::Box;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Not owned by this CPU.
    Free,
    /// Only this CPU's idle thread is ever in this state.
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
    idle: Tid,
    current: Tid,
    policy: Box<dyn SchedPolicy>,
    fallback: RoundRobin,
    on_fallback: bool,
    slice_start: u64,
    watchdog_ns: u64,
    ejections: u32,
    pending_ejection: Option<&'static str>,
    switches: u64,
    /// A wake-up asked for a reschedule that no switch has consumed yet.
    resched_pending: bool,
}

impl Core {
    /// A CPU starts out running its idle thread.
    pub fn new(policy: Box<dyn SchedPolicy>, idle: Tid, watchdog_ns: u64, now: u64) -> Self {
        let mut states = [State::Free; MAX_THREADS];
        let mut names = [""; MAX_THREADS];
        states[idle] = State::Idle;
        names[idle] = "idle";
        Self {
            states,
            names,
            idle,
            current: idle,
            policy,
            fallback: RoundRobin::new(),
            on_fallback: false,
            slice_start: now,
            watchdog_ns,
            ejections: 0,
            pending_ejection: None,
            switches: 0,
            resched_pending: false,
        }
    }

    pub fn current(&self) -> Tid {
        self.current
    }

    pub fn idle(&self) -> Tid {
        self.idle
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

    /// The policy last installed, even while the watchdog has the fallback running.
    pub fn installed_policy_name(&self) -> &'static str {
        self.policy.name()
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

    /// Runnable plus running threads, not counting idle. Used for load balancing.
    pub fn load(&self) -> usize {
        self.states
            .iter()
            .filter(|s| matches!(s, State::Runnable { .. } | State::Running))
            .count()
    }

    /// The reason for the latest ejection, once, so the caller can log it.
    pub fn take_ejection(&mut self) -> Option<&'static str> {
        self.pending_ejection.take()
    }

    /// Makes `tid` the running thread; used once for the code that boots the CPU.
    pub fn adopt_running(&mut self, tid: Tid, name: &'static str) {
        self.names[tid] = name;
        self.states[tid] = State::Running;
        self.current = tid;
    }

    /// Takes ownership of a runnable thread: a new one, or one migrated here.
    pub fn adopt(&mut self, tid: Tid, name: &'static str, reason: EnqueueReason, now: u64) {
        self.names[tid] = name;
        self.make_runnable(tid, reason, now);
    }

    /// Picks a free slot and adopts it as a new thread. Convenient for single-CPU use.
    pub fn spawn(&mut self, name: &'static str, now: u64) -> Option<Tid> {
        let tid = (0..MAX_THREADS).find(|&t| t != self.idle && self.states[t] == State::Free)?;
        self.adopt(tid, name, EnqueueReason::New, now);
        Some(tid)
    }

    /// Gives up the longest-waiting runnable thread that `skip` allows, for migration
    /// to another CPU. The caller must not move a thread that is still on a CPU.
    pub fn detach_runnable(&mut self, skip: impl Fn(Tid) -> bool) -> Option<(Tid, &'static str)> {
        let (_, tid) = self
            .states
            .iter()
            .enumerate()
            .filter_map(|(tid, state)| match state {
                State::Runnable { since } if !skip(tid) => Some((*since, tid)),
                _ => None,
            })
            .min()?;
        self.active().dequeue(tid);
        self.states[tid] = State::Free;
        Some((tid, self.names[tid]))
    }

    /// Frees an exited thread's slot once it is completely off the CPU.
    pub fn release(&mut self, tid: Tid) -> bool {
        if self.state(tid) == State::Dead {
            self.states[tid] = State::Free;
            true
        } else {
            false
        }
    }

    /// Timer tick: wakes sleepers, runs the watchdog, asks the policy about preemption.
    /// Returns the thread to switch to, if the current one should change.
    pub fn tick(&mut self, now: u64) -> Option<Tid> {
        // A wake-up whose reschedule interrupt has not arrived yet is delivered here.
        let mut reschedule = self.resched_pending;
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
        if self.current == self.idle {
            reschedule |= self.any_runnable();
        } else {
            let slice = now.saturating_sub(self.slice_start);
            let current = self.current;
            reschedule |= self.active().tick(current, slice);
        }
        if reschedule { self.preempt(now) } else { None }
    }

    /// Wakes a blocked or sleeping thread this CPU owns. Returns true if the current
    /// thread should be preempted for it; the owner CPU then calls
    /// [`Core::preempt_if_pending`], possibly later from an interrupt.
    pub fn wake(&mut self, tid: Tid, now: u64) -> bool {
        match self.state(tid) {
            State::Blocked | State::Sleeping { .. } => {}
            _ => return false,
        }
        self.make_runnable(tid, EnqueueReason::Woken, now);
        let wants = self.should_preempt_for(tid);
        self.resched_pending |= wants;
        wants
    }

    /// Like [`Core::preempt`], but only if a wake-up asked for it and no switch has
    /// consumed it since, or if this CPU is idle with work queued. A late reschedule
    /// interrupt therefore cannot demote a woken thread that is already running.
    pub fn preempt_if_pending(&mut self, now: u64) -> Option<Tid> {
        let idle_with_work = self.current == self.idle && self.any_runnable();
        if self.resched_pending || idle_with_work {
            self.preempt(now)
        } else {
            None
        }
    }

    /// Puts the running thread back in the queue and picks again.
    /// Returns the new thread if it differs from the current one.
    pub fn preempt(&mut self, now: u64) -> Option<Tid> {
        let previous = self.current;
        if previous != self.idle && self.states[previous] == State::Running {
            self.make_runnable(previous, EnqueueReason::Preempted, now);
        }
        let next = self.pick(now);
        (next != previous).then_some(next)
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
        if self.current != self.idle {
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
        if self.current != self.idle {
            self.states[self.current] = state;
        }
        self.pick(now)
    }

    fn should_preempt_for(&self, woken: Tid) -> bool {
        self.current == self.idle
            || if self.on_fallback {
                self.fallback.preempt_on_wake(woken, self.current)
            } else {
                self.policy.preempt_on_wake(woken, self.current)
            }
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
                None => break self.idle,
            }
        };
        if next != self.idle {
            self.states[next] = State::Running;
        }
        if next != self.current {
            self.switches += 1;
        }
        self.current = next;
        self.resched_pending = false;
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
        tid != self.idle && matches!(self.state(tid), State::Runnable { .. })
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
mod tests;
