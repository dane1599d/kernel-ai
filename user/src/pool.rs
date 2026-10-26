//! A thread pool for the engine (L6): one worker pinned on each CPU, up to a limit,
//! and [`Pool::broadcast`], which runs a job once on every worker and returns when
//! all of them have. The engine's `Parallel` trait can sit on it: a matvec gives
//! each worker the rows [`share`] says, so the result does not depend on how many
//! workers there are or which finishes first.
//!
//! Between jobs a worker spins briefly on the job counter, then yields the CPU
//! while it looks (so a caller woken on its CPU runs at once), and only after
//! [`YIELDS`] of those sleeps on it with FUTEX_WAIT. The caller yields while it
//! waits for the count of finished workers in the same way. A decode step makes a
//! few hundred broadcasts a token, and in a KVM guest a futex sleep and wake cost
//! about 370 us each way (S02-2), against a few microseconds for a yield. The
//! process needs
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
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use kabi::Error;
use kabi::thread::MAX_WORKERS;

/// Stack pages for each worker: 64 KiB.
pub const STACK_PAGES: u64 = 16;
/// Looks at a counter with PAUSE before yielding while it waits.
const SPINS: u32 = 64;
/// Yields while waiting before sleeping on the futex: about a millisecond or two
/// in a guest, longer than the gap between a decode step's broadcasts.
const YIELDS: u32 = 1_000;
/// A worker that has not blocked for this long sleeps a millisecond before its
/// next wait: a yield is not blocking, and the kernel kills a thread that runs
/// its process's burst limit (10 s for llm) without blocking outside a call.
const BLOCK_EVERY_MS: u64 = 1_000;
/// How long `Pool::new` retries a Busy THREAD_SPAWN: a worker that just exited
/// holds its index until the kernel has let its thread go.
const BUSY_RETRY_MS: u64 = 1_000;

/// What broadcasts cost since [`Pool::take_stats`], in TSC cycles: the caller's
/// time from start to all finished, and of that each broadcast's slowest
/// worker's time in the job. The difference is the hand-off.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub broadcasts: u64,
    pub cycles: u64,
    pub slowest: u64,
}

/// The time-stamp counter: cheap in a guest, where the TIME call may exit.
pub fn cycles() -> u64 {
    // SAFETY: RDTSC reads a counter and has no other effect; ring 3 may run it.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// TSC cycles in a millisecond, over 10 ms of the TIME call's clock.
fn cycles_per_ms() -> u64 {
    let (ns, tsc) = (sys::time_ns(), cycles());
    sys::sleep_ms(10);
    let (ns, tsc) = (sys::time_ns() - ns, cycles() - tsc);
    (tsc.saturating_mul(1_000_000) / ns.max(1)).max(1)
}

/// A job: `(worker, workers)`, borrowing whatever it needs for `'a`.
type Job<'a> = dyn Fn(usize, usize) + Sync + 'a;

struct Shared {
    /// Bumped for every job, and once more to stop; workers sleep on it.
    generation: AtomicU32,
    /// Workers done with the current job; the caller sleeps on it.
    finished: AtomicU32,
    /// Workers started and not yet about to exit; `Drop` sleeps on it.
    live: AtomicU32,
    /// Workers asleep (or about to be) on `generation`; a broadcast wakes them
    /// only when there are some.
    sleepers: AtomicU32,
    /// Set while the caller sleeps (or is about to) on `finished`.
    caller_asleep: AtomicBool,
    stop: AtomicBool,
    /// Set while a broadcast runs, to catch a second one at run time too.
    busy: AtomicBool,
    workers: usize,
    /// TSC cycles in [`BLOCK_EVERY_MS`], measured at start.
    block_every: u64,
    /// Each worker's cycles in the current job, written before it counts itself
    /// finished.
    job_cycles: [AtomicU64; MAX_WORKERS],
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
    stats: Cell<Stats>,
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
        let block_every = cycles_per_ms().saturating_mul(BLOCK_EVERY_MS);
        let shared: &'static Shared = Box::leak(Box::new(Shared {
            generation: AtomicU32::new(0),
            finished: AtomicU32::new(0),
            live: AtomicU32::new(0),
            sleepers: AtomicU32::new(0),
            caller_asleep: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            workers,
            block_every,
            job_cycles: [const { AtomicU64::new(0) }; MAX_WORKERS],
            job: UnsafeCell::new(None),
        }));
        let pool = Pool {
            shared,
            stats: Cell::new(Stats::default()),
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

    /// What broadcasts cost since the last call, which starts the count again.
    pub fn take_stats(&self) -> Stats {
        self.stats.replace(Stats::default())
    }

    /// Runs `job(worker, workers)` once on every worker, and returns when every one
    /// has finished. A panic in the job ends the whole process.
    pub fn broadcast(&self, job: &Job<'_>) {
        let shared = self.shared;
        assert!(
            !shared.busy.swap(true, Ordering::Acquire),
            "pool: a broadcast while another runs"
        );
        let started = cycles();
        // SAFETY: the reference lives until this call returns, and no worker uses it
        // after counting itself finished, which this call waits for below.
        let job: &'static Job<'static> =
            unsafe { core::mem::transmute::<&Job<'_>, &'static Job<'static>>(job) };
        // SAFETY: every worker finished the previous job (or none ran yet), so none
        // reads the cell now; the generation bump below publishes the write.
        unsafe { *shared.job.get() = Some(job) };
        shared.finished.store(0, Ordering::Release);
        // SeqCst with the sleepers count: a worker that counts itself asleep
        // after this bump still sees it before it sleeps.
        shared.generation.fetch_add(1, Ordering::SeqCst);
        if shared.sleepers.load(Ordering::SeqCst) > 0 {
            let _ = sys::futex_wake(&shared.generation, u32::MAX);
        }
        let workers = shared.workers as u32;
        let mut waits = 0;
        loop {
            let finished = shared.finished.load(Ordering::Acquire);
            if finished == workers {
                break;
            }
            waits += 1;
            match waits {
                n if n <= SPINS => core::hint::spin_loop(),
                n if n <= SPINS + YIELDS => sys::yield_now(),
                _ => {
                    // SeqCst with the last worker's count, as for `sleepers`.
                    shared.caller_asleep.store(true, Ordering::SeqCst);
                    if shared.finished.load(Ordering::SeqCst) == finished {
                        let _ = sys::futex_wait(&shared.finished, finished, 0);
                    }
                    shared.caller_asleep.store(false, Ordering::SeqCst);
                }
            }
        }
        // SAFETY: as above, every worker is done with it.
        unsafe { *shared.job.get() = None };
        let slowest = shared.job_cycles[..shared.workers]
            .iter()
            .map(|c| c.load(Ordering::Relaxed))
            .max()
            .unwrap_or(0);
        let Stats {
            broadcasts,
            cycles: total,
            slowest: slow,
        } = self.stats.get();
        self.stats.set(Stats {
            broadcasts: broadcasts + 1,
            cycles: total + (cycles() - started),
            slowest: slow + slowest,
        });
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
    for wait in 0..SPINS + YIELDS {
        let now = shared.generation.load(Ordering::Acquire);
        if now != seen {
            return now;
        }
        match wait < SPINS {
            true => core::hint::spin_loop(),
            false => sys::yield_now(),
        }
    }
    loop {
        shared.sleepers.fetch_add(1, Ordering::SeqCst);
        let now = shared.generation.load(Ordering::SeqCst);
        if now == seen {
            let _ = sys::futex_wait(&shared.generation, seen, 0);
        }
        shared.sleepers.fetch_sub(1, Ordering::SeqCst);
        let now = shared.generation.load(Ordering::Acquire);
        if now != seen {
            return now;
        }
    }
}

extern "C" fn worker_main(arg: u64) -> ! {
    // SAFETY: `arg` is the leaked `Start` that `Pool::new` made for this worker.
    let start = unsafe { &*(arg as *const Start) };
    let shared = start.shared;
    let mut seen = 0;
    let mut blocked = cycles();
    loop {
        if cycles() - blocked > shared.block_every {
            sys::sleep_ms(1);
            blocked = cycles();
        }
        seen = next_generation(shared, seen);
        if shared.stop.load(Ordering::Acquire) {
            shared.live.fetch_sub(1, Ordering::AcqRel);
            let _ = sys::futex_wake(&shared.live, 1);
            let refused = sys::thread_exit();
            crate::println!("pool: a worker could not exit: {refused:?}");
            sys::exit(102);
        }
        let began = cycles();
        // SAFETY: a new generation was published after the job was written, and the
        // caller waits for this worker before it changes the cell again.
        if let Some(job) = unsafe { *shared.job.get() } {
            job(start.index, shared.workers);
        }
        shared.job_cycles[start.index].store(cycles() - began, Ordering::Relaxed);
        let done = shared.finished.fetch_add(1, Ordering::SeqCst) + 1;
        if done as usize == shared.workers && shared.caller_asleep.load(Ordering::SeqCst) {
            let _ = sys::futex_wake(&shared.finished, 1);
        }
    }
}
