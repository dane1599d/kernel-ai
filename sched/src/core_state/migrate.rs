//! Migration that keeps a thread's accounting: the wait it spent here and whether
//! it woke go with it, so the destination records the whole wake-to-run time. The
//! destination's watchdog still counts only the wait on its own CPU, since the
//! source's wait is not its policy's doing. A thread that woke also keeps its
//! boost there and may take that CPU as a wake there would; its wake is counted
//! once, where it woke.

use super::{Core, EnqueueReason, State, Tid};

/// A runnable thread taken off one Core, to be adopted by another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detached {
    pub tid: Tid,
    pub name: &'static str,
    waited_ns: u64,
    woken: bool,
}

impl Core {
    /// Like [`Core::detach_runnable`], but carrying the thread's accounting.
    pub fn detach_waiting(&mut self, skip: impl Fn(Tid) -> bool, now: u64) -> Option<Detached> {
        let (tid, name, since) = self.detach_oldest(skip)?;
        let waited_ns = now.saturating_sub(since);
        let woken = self.stats.left(tid, waited_ns);
        Some(Detached {
            tid,
            name,
            waited_ns,
            woken,
        })
    }

    /// Gives up a blocked or sleeping thread, for a waker on another CPU that
    /// adopts it there as woken (the kernel's IPC handoff, `threads::wake_here`).
    /// Unlike a wake here and then a detach, it counts no wake on this CPU and
    /// leaves no reschedule pending for a thread that is gone. None, and
    /// nothing changed, if `tid` is not asleep here. The caller must not take a
    /// thread whose registers are not saved yet.
    pub fn detach_asleep(&mut self, tid: Tid) -> Option<&'static str> {
        if !matches!(self.state(tid), State::Blocked | State::Sleeping { .. }) {
            return None;
        }
        self.set_state(tid, State::Free);
        Some(self.names[tid])
    }

    /// Adopts a thread from [`Core::detach_waiting`]: queued as woken if it
    /// woke, so it keeps the boost it is owed (as a policy switch hands it
    /// over), without counting its wake again. Returns true, as [`Core::wake`]
    /// does, if it should preempt the running thread; the caller then calls
    /// [`Core::preempt_if_pending`], which also runs it on an idle CPU.
    pub fn adopt_detached(&mut self, thread: Detached, now: u64) -> bool {
        let tid = thread.tid;
        let reason = match thread.woken {
            true => EnqueueReason::Woken,
            false => EnqueueReason::Preempted,
        };
        self.names[tid] = thread.name;
        self.set_state(tid, State::Runnable { since: now });
        self.woke[tid] = thread.woken;
        self.stats.arrived(tid, thread.woken, thread.waited_ns);
        self.active().enqueue(tid, reason);
        let wants = thread.woken && self.should_preempt_for(tid);
        self.resched_pending |= wants;
        wants
    }
}
