//! The engine's workers: `user::pool`'s threads, one pinned on each CPU online,
//! as the `quant::Parallel` the engine spreads its matrix rows and attention
//! heads over. The pool lives as long as the service, so the engine borrows it
//! for `'static`. A kill takes the workers with the process; the kernel wakes
//! each one to die.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use quant::{Parallel, Serial};
use user::pool::{self, Pool};
use user::{println, sys};

struct Workers(Pool);

impl Parallel for Workers {
    fn workers(&self) -> usize {
        self.0.workers()
    }

    fn broadcast(&self, job: &(dyn Fn(usize, usize) + Sync)) {
        self.0.broadcast(job);
    }
}

/// The pool once started, for [`report`]; only the service's own thread uses it.
static POOL: AtomicPtr<Workers> = AtomicPtr::new(core::ptr::null_mut());
/// TSC cycles per microsecond, measured at start.
static CYCLES_PER_US: AtomicU64 = AtomicU64::new(0);
/// Empty broadcasts timed at start.
const EMPTY: u64 = 1_000;

/// A pool of up to `kabi::thread::MAX_WORKERS` workers, or the service's own
/// thread alone when the kernel starts none. It logs what one empty broadcast
/// costs: the hand-off every product pays.
pub fn start() -> &'static dyn Parallel {
    match Pool::new(kabi::thread::MAX_WORKERS) {
        Ok(pool) => {
            let workers: &'static Workers = Box::leak(Box::new(Workers(pool)));
            calibrate();
            for _ in 0..EMPTY {
                workers.0.broadcast(&|_, _| {});
            }
            let stats = workers.0.take_stats();
            println!(
                "local: {} workers; an empty broadcast takes {:.1} us",
                workers.0.workers(),
                us(stats.cycles) / EMPTY as f64
            );
            POOL.store(workers as *const Workers as *mut Workers, Ordering::Release);
            workers
        }
        Err(error) => {
            println!("local: no worker threads ({error:?}); the engine runs on one");
            &Serial
        }
    }
}

/// TSC cycles over 20 ms of the TIME call's clock.
fn calibrate() {
    let (ns, tsc) = (sys::time_ns(), pool::cycles());
    sys::sleep_ms(20);
    let (ns, tsc) = (sys::time_ns() - ns, pool::cycles() - tsc);
    CYCLES_PER_US.store((tsc * 1000 / ns.max(1)).max(1), Ordering::Relaxed);
}

/// `cycles` of the TSC in milliseconds.
pub fn ms(cycles: u64) -> u64 {
    (us(cycles) / 1000.0) as u64
}

fn us(cycles: u64) -> f64 {
    cycles as f64 / CYCLES_PER_US.load(Ordering::Relaxed).max(1) as f64
}

/// What the pool's broadcasts cost since the last report, for S02-2's profile:
/// `412 broadcasts, 900 ms, 60 ms of hand-off`, or "" with no pool.
pub fn report() -> String {
    let pool = POOL.load(Ordering::Acquire);
    if pool.is_null() {
        return String::new();
    }
    // SAFETY: a non-null pointer is the leaked `Workers`, which lives for ever,
    // and only the service's thread, which broadcasts, reads its stats.
    let stats = unsafe { &*pool }.0.take_stats();
    format!(
        "{} broadcasts, {} ms, {} ms of hand-off",
        stats.broadcasts,
        ms(stats.cycles),
        ms(stats.cycles - stats.slowest.min(stats.cycles))
    )
}
