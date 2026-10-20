//! A thread pool for the engine (L6): one worker pinned on each CPU, up to a limit,
//! and [`Pool::broadcast`], which runs a job once on every worker and returns when
//! all of them have. The engine's `Parallel` trait can sit on it: a matvec gives
//! each worker the rows [`share`] says, so the result does not depend on how many
//! workers there are or which finishes first.
//!
//! Between jobs a worker spins briefly on the job counter, then sleeps on it with
//! FUTEX_WAIT; the caller sleeps on the count of finished workers. The process needs
//! `cap::THREADS`, and its quota pays for each worker's stack. A pool is meant to
//! live as long as its process: dropping it waits for its workers to exit, and its
//! few hundred bytes of shared state are not given back.
//!
//! A `Pool` is not `Sync`, so a job cannot capture one and broadcast from inside a
//! job; one broadcast at a time is what makes the shared job cell safe.

use crate::sys;
use alloc::boxed::Box;
use core::cell::{Cell, UnsafeCell};
use core::marker::PhantomData;
use core::ops::Range;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use kabi::Error;
use kabi::thread::MAX_WORKERS;

/// Stack pages for each worker: 64 KiB.
pub const STACK_PAGES: u64 = 16;
/// Looks at the job counter before a worker goes to sleep on it.
const SPINS: u32 = 2_000;
/// How long `Pool::new` retries a Busy THREAD_SPAWN: a worker that just exited
/// holds its index until the kernel has let its thread go.
const BUSY_RETRY_MS: u64 = 1_000;

/// A job: `(worker, workers)`, borrowing whatever it needs for `'a`.
type Job<'a> = dyn Fn(usize, usize) + Sync + 'a;

struct Shared {
    /// Bumped for every job, and once more to stop; workers sleep on it.
    generation: AtomicU32,
    /// Workers done with the current job; the caller sleeps on it.
    finished: AtomicU32,
    /// Workers started and not yet about to exit; `Drop` sleeps on it.
    live: AtomicU32,
    stop: AtomicBool,
    /// Set while a broadcast runs, to catch a second one at run time too.
    busy: AtomicBool,
    workers: usize,
    /// The current job, set by the caller only while no worker runs one.
    job: UnsafeCell<Option<&'static Job<'static>>>,
}

// SAFETY: `job` is written only by `broadcast` between jobs, when every worker has
// counted itself finished and none reads it, and read by workers only after seeing a
// new generation, which `broadcast` publishes with Release after writing it. Only one
// broadcast runs at a time: `Pool` is not Sync, so a job cannot reach it, and `busy`
// stops a second one anyway.
unsafe impl Sync for Shared {}

/// One worker's start argument.
struct Start {
    shared: &'static Shared,
    index: usize,
}

pub struct Pool {
    shared: &'static Shared,
    /// Not Sync: a job (which must be Sync) can never hold the pool.
    _not_sync: PhantomData<Cell<()>>,
}

/// The range of `total` items worker `worker` of `workers` takes: contiguous, as
/// even as can be, the first `total % workers` workers taking one more.
pub fn share(total: usize, worker: usize, workers: usize) -> Range<usize> {
    let workers = workers.max(1);
    let (base, extra) = (total / workers, total % workers);
    let start = worker * base + worker.min(extra);
    start..start + base + usize::from(worker < extra)
}

/// THREAD_SPAWN for a pool worker, retried for a moment while it says Busy.
fn spawn(arg: u64, cpu: usize) -> Result<usize, Error> {
    let deadline = sys::time_ns() + BUSY_RETRY_MS * 1_000_000;
    loop {
        match sys::thread_spawn(worker_main, arg, cpu, STACK_PAGES) {
            Err(Error::Busy) if sys::time_ns() < deadline => sys::sleep_ms(1),
            other => return other,
        }
    }
}

impl Pool {
    /// Starts one worker pinned on each CPU online, at most `max` (and at most
    /// `kabi::thread::MAX_WORKERS`), at least one.
    pub fn new(max: usize) -> Result<Pool, Error> {
        let cpus = sys::metrics().map_or(1, |metrics| metrics.cpus as usize);
        let workers = cpus.min(max).clamp(1, MAX_WORKERS);
        let shared: &'static Shared = Box::leak(Box::new(Shared {
            generation: AtomicU32::new(0),
            finished: AtomicU32::new(0),
            live: AtomicU32::new(0),
            stop: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            workers,
            job: UnsafeCell::new(None),
        }));
        let pool = Pool {
            shared,
            _not_sync: PhantomData,
        };
        for index in 0..workers {
            let start: &'static Start = Box::leak(Box::new(Start { shared, index }));
            shared.live.fetch_add(1, Ordering::AcqRel);
            if let Err(error) = spawn(start as *const Start as u64, index % cpus) {
                shared.live.fetch_sub(1, Ordering::AcqRel);
                // Dropping `pool` stops and waits for the ones already started.
                return Err(error);
            }
        }
        Ok(pool)
    }

    /// Workers in the pool.
    pub fn workers(&self) -> usize {
        self.shared.workers
    }

    /// Runs `job(worker, workers)` once on every worker, and returns when every one
    /// has finished. A panic in the job ends the whole process.
    pub fn broadcast(&self, job: &Job<'_>) {
        let shared = self.shared;
        assert!(
            !shared.busy.swap(true, Ordering::Acquire),
            "pool: a broadcast while another runs"
        );
        // SAFETY: the reference lives until this call returns, and no worker uses it
        // after counting itself finished, which this call waits for below.
        let job: &'static Job<'static> =
            unsafe { core::mem::transmute::<&Job<'_>, &'static Job<'static>>(job) };
        // SAFETY: every worker finished the previous job (or none ran yet), so none
        // reads the cell now; the generation bump below publishes the write.
        unsafe { *shared.job.get() = Some(job) };
        shared.finished.store(0, Ordering::Release);
        shared.generation.fetch_add(1, Ordering::Release);
        let _ = sys::futex_wake(&shared.generation, u32::MAX);
        let workers = shared.workers as u32;
        loop {
            let finished = shared.finished.load(Ordering::Acquire);
            if finished == workers {
                break;
            }
            let _ = sys::futex_wait(&shared.finished, finished, 0);
        }
        // SAFETY: as above, every worker is done with it.
        unsafe { *shared.job.get() = None };
        shared.busy.store(false, Ordering::Release);
    }
}

impl Drop for Pool {
    /// Stops the workers and waits until each is about to exit.
    fn drop(&mut self) {
        let shared = self.shared;
        shared.stop.store(true, Ordering::Release);
        shared.generation.fetch_add(1, Ordering::Release);
        let _ = sys::futex_wake(&shared.generation, u32::MAX);
        loop {
            let live = shared.live.load(Ordering::Acquire);
            if live == 0 {
                break;
            }
            let _ = sys::futex_wait(&shared.live, live, 0);
        }
    }
}

/// Waits until the job counter moves past `seen`; returns its new value.
fn next_generation(shared: &Shared, seen: u32) -> u32 {
    for _ in 0..SPINS {
        let now = shared.generation.load(Ordering::Acquire);
        if now != seen {
            return now;
        }
        core::hint::spin_loop();
    }
    loop {
        let now = shared.generation.load(Ordering::Acquire);
        if now != seen {
            return now;
        }
        let _ = sys::futex_wait(&shared.generation, seen, 0);
    }
}

extern "C" fn worker_main(arg: u64) -> ! {
    // SAFETY: `arg` is the leaked `Start` that `Pool::new` made for this worker.
    let start = unsafe { &*(arg as *const Start) };
    let shared = start.shared;
    let mut seen = 0;
    loop {
        seen = next_generation(shared, seen);
        if shared.stop.load(Ordering::Acquire) {
            shared.live.fetch_sub(1, Ordering::AcqRel);
            let _ = sys::futex_wake(&shared.live, 1);
            let refused = sys::thread_exit();
            crate::println!("pool: a worker could not exit: {refused:?}");
            sys::exit(102);
        }
        // SAFETY: a new generation was published after the job was written, and the
        // caller waits for this worker before it changes the cell again.
        if let Some(job) = unsafe { *shared.job.get() } {
            job(start.index, shared.workers);
        }
        let done = shared.finished.fetch_add(1, Ordering::AcqRel) + 1;
        if done as usize == shared.workers {
            let _ = sys::futex_wake(&shared.finished, 1);
        }
    }
}
