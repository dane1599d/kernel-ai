//! Migration that keeps a thread's accounting: the wait it spent here and whether
//! it woke go with it, so the destination records the whole wake-to-run time. The
//! destination's watchdog still counts only the wait on its own CPU, since the
//! source's wait is not its policy's doing.

use super::{Core, EnqueueReason, Tid};

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

    /// Adopts a thread from [`Core::detach_waiting`].
    pub fn adopt_detached(&mut self, thread: Detached, now: u64) {
        self.adopt(thread.tid, thread.name, EnqueueReason::Preempted, now);
        self.stats
            .arrived(thread.tid, thread.woken, thread.waited_ns);
    }
}
