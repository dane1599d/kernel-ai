//! Worker threads (L6): up to `kabi::thread::MAX_WORKERS` threads beside a
//! process's main thread, each a kernel thread pinned to one CPU that runs in the
//! process's address space on a stack of its own (see `kabi::thread`).
//!
//! - Only the main thread of a process holding `cap::THREADS` starts workers, and a
//!   worker may make only the calls [`worker_may`] lists, so IPC, supervision and
//!   the model stay single-threaded as before.
//! - THREAD_SPAWN counts the worker in `live_threads` before its thread exists, so
//!   the process cannot be torn down under it. The worker registers its tid under
//!   the TABLE lock and only then looks for a kill; `kill` records its reason and
//!   copies the tids in one TABLE critical section. Either the copy has the worker,
//!   or the worker sees the kill before it ever reaches ring 3.
//! - Stack pages are never unmapped while the process lives (no shootdown is
//!   needed); a later worker with the same index reuses them.

use super::{MAX_PROCESSES, Process, SLOT_OF_TID, SPACES, State, exit_current, killed, with_table};
use crate::memory::user::{Access, MIN_USER_ADDRESS};
use crate::{entry, memory, smp, threads};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use kabi::layout::USER_END;
use kabi::thread::{MAX_STACK_PAGES, MAX_WORKERS, stack_bottom, stack_top};
use kabi::{Error, call};
use mm::PAGE_SIZE;
use sched::{MAX_THREADS, Tid};
use x86_64::instructions::interrupts;

/// One worker index of a process.
#[derive(Clone, Copy)]
pub struct Worker {
    /// The worker's thread, once it has started.
    pub tid: Option<Tid>,
    /// Taken by THREAD_SPAWN until the worker's thread exits.
    pub reserved: bool,
    /// Pages mapped at the top of this index's stack window; kept for the next worker.
    pub stack_pages: u64,
    /// Where it enters ring 3, and its argument (RDI).
    pub entry: u64,
    pub arg: u64,
    /// The CPU it is pinned to.
    pub cpu: usize,
}

impl Worker {
    pub const FREE: Worker = Worker {
        tid: None,
        reserved: false,
        stack_pages: 0,
        entry: 0,
        arg: 0,
        cpu: 0,
    };
}

/// Every thread of a process: the main thread, then the workers.
pub type Tids = [Option<Tid>; 1 + MAX_WORKERS];

/// Set while a thread is a process's worker, for the call filter.
static IS_WORKER: [AtomicBool; MAX_THREADS] = [const { AtomicBool::new(false) }; MAX_THREADS];
/// Workers that found themselves on another CPU than the one they were pinned to.
static MISPLACED: AtomicUsize = AtomicUsize::new(0);

/// How many workers started on a CPU other than theirs, for the tests: always 0.
pub fn misplaced() -> usize {
    MISPLACED.load(Ordering::SeqCst)
}

/// Whether `tid` is a worker thread.
pub fn is_worker(tid: Tid) -> bool {
    IS_WORKER[tid].load(Ordering::Acquire)
}

/// The calls a worker may make: its own end, the process's end, output, time,
/// sleeping and yielding, heap growth, the metrics, and futexes. Everything else
/// is the main thread's.
pub fn worker_may(number: u64) -> bool {
    matches!(
        number,
        call::EXIT
            | call::WRITE
            | call::SLEEP
            | call::YIELD
            | call::BRK
            | call::TIME
            | call::METRICS
            | call::THREAD_EXIT
            | call::FUTEX_WAIT
            | call::FUTEX_WAKE
    )
}

impl Process {
    /// How many threads the process has running now.
    pub fn thread_count(&self) -> usize {
        self.thread_tids().iter().flatten().count()
    }

    /// Every thread the process has now.
    pub fn thread_tids(&self) -> Tids {
        let mut tids = [None; 1 + MAX_WORKERS];
        tids[0] = self.tid;
        for (tid, worker) in tids[1..].iter_mut().zip(&self.workers) {
            *tid = worker.tid;
        }
        tids
    }

    /// Drops `tid` from the process's threads, freeing its worker index.
    pub(super) fn forget_thread(&mut self, tid: Tid) {
        if self.tid == Some(tid) {
            self.tid = None;
        } else if let Some(worker) = self.workers.iter_mut().find(|w| w.tid == Some(tid)) {
            worker.tid = None;
            worker.reserved = false;
        }
    }
}

/// Wakes each of `tids`; a thread that is not blocked is not disturbed.
pub fn wake_all(tids: &Tids) {
    interrupts::without_interrupts(|| threads::wake_all(tids.iter().flatten().copied()));
}

/// The THREAD_SPAWN call.
pub fn spawn(slot: usize, entry: u64, arg: u64, cpu: u64, stack_pages: u64) -> Result<u64, Error> {
    let me = threads::current_tid();
    let user = MIN_USER_ADDRESS..USER_END - PAGE_SIZE;
    let cpu = usize::try_from(cpu).map_err(|_| Error::Invalid)?;
    if !user.contains(&entry) || cpu >= smp::online() {
        return Err(Error::Invalid);
    }
    if !(1..=MAX_STACK_PAGES).contains(&stack_pages) {
        return Err(Error::Invalid);
    }
    let (index, mapped, max_pages) = with_table(|table| {
        let process = &mut table[slot];
        if process.tid != Some(me) {
            return Err(Error::Permission);
        }
        if process.state != State::Running || killed(slot) {
            return Err(Error::Dead);
        }
        let index = process
            .workers
            .iter()
            .position(|w| !w.reserved)
            .ok_or(Error::Busy)?;
        let worker = &mut process.workers[index];
        *worker = Worker {
            tid: None,
            reserved: true,
            entry,
            arg,
            cpu,
            ..*worker
        };
        process.live_threads += 1;
        Ok((index, worker.stack_pages, process.limits.max_pages))
    })?;
    let started = map_stack(slot, index, mapped, stack_pages, max_pages).and_then(|()| {
        threads::spawn_pinned(cpu, "worker", worker_main, slot * MAX_WORKERS + index)
            .map(|_| ())
            .map_err(|error| match error {
                "threads: table full" => Error::Busy,
                _ => Error::NoMemory,
            })
    });
    if let Err(error) = started {
        with_table(|table| {
            table[slot].workers[index].reserved = false;
            table[slot].live_threads -= 1;
        });
        return Err(error);
    }
    Ok(index as u64)
}

/// Maps the stack pages worker `index` needs beyond the `mapped` it has, downwards
/// from the bottom of what is there, and records each page as it goes. Past the
/// quota it stops with NoMemory and kills nothing.
fn map_stack(
    slot: usize,
    index: usize,
    mapped: u64,
    pages: u64,
    max_pages: u64,
) -> Result<(), Error> {
    let stack = Access {
        write: true,
        execute: false,
    };
    let mut guard = SPACES[slot].lock();
    let space = guard.as_mut().ok_or(Error::Dead)?;
    for have in mapped..pages {
        if space.frames() >= max_pages {
            return Err(Error::NoMemory);
        }
        let page = stack_bottom(index, have + 1);
        memory::with(|m| m.map_user_page(space, page, stack)).map_err(|_| Error::NoMemory)?;
        with_table(|table| table[slot].workers[index].stack_pages = have + 1);
        if space.frames() > max_pages {
            return Err(Error::NoMemory);
        }
    }
    Ok(())
}

/// The worker's kernel thread: register, then enter ring 3 with clean vector state.
/// THREAD_SPAWN counted it in `live_threads`, so its slot cannot have been torn
/// down or reused; finding its index not reserved, or taken, would be a kernel bug,
/// and the thread then ends without touching the slot.
fn worker_main(packed: usize) {
    let (slot, index) = (packed / MAX_WORKERS, packed % MAX_WORKERS);
    let me = threads::current_tid();
    let found = (slot < MAX_PROCESSES).then(|| {
        with_table(|table| {
            let process = &mut table[slot];
            let (live, cr3) = (process.live(), process.cr3);
            let worker = &mut process.workers[index];
            if !live || !worker.reserved || worker.tid.is_some() {
                return None;
            }
            worker.tid = Some(me);
            Some((worker.entry, worker.arg, worker.cpu, cr3))
        })
    });
    let Some(Some((entry, arg, cpu, cr3))) = found else {
        crate::log!("process: a worker found its index {index} in slot {slot} not reserved");
        return;
    };
    // Pinned, so this cannot change under it.
    if crate::percpu::index() != cpu {
        MISPLACED.fetch_add(1, Ordering::SeqCst);
    }
    IS_WORKER[me].store(true, Ordering::Release);
    // Before the slot is published, from when on this thread is charged.
    super::burst::start_thread(me);
    SLOT_OF_TID[me].store(slot + 1, Ordering::Release);
    super::thread_started(me, slot);
    // After registering: a kill that came before is seen here, and one that comes
    // after has this tid to wake.
    if killed(slot) {
        exit_current();
    }
    threads::enter_address_space(cr3);
    crate::fpu::enter_ring3();
    // SAFETY: the process's space is loaded and holds the stack window mapped by
    // THREAD_SPAWN; `entry` is a user address (checked there), and a bad one only
    // faults in ring 3, killing the process. Nothing on this stack needs dropping.
    unsafe { entry::enter_user(entry, stack_top(index) - 8, arg) }
}

/// The THREAD_EXIT call: ends a worker; the main thread is refused.
pub fn exit_worker() -> Result<u64, Error> {
    if !is_worker(threads::current_tid()) {
        return Err(Error::Invalid);
    }
    exit_current()
}

/// Called by every exiting process thread: it is no longer a worker.
pub(super) fn forget(tid: Tid) {
    IS_WORKER[tid].store(false, Ordering::Release);
}
