//! The engine's workers: `user::pool`'s threads, one pinned on each CPU online,
//! as the `quant::Parallel` the engine spreads its matrix rows and attention
//! heads over. The pool lives as long as the service, so the engine borrows it
//! for `'static`. A kill takes the workers with the process; the kernel wakes
//! each one to die.

use alloc::boxed::Box;
use quant::{Parallel, Serial};
use user::pool::Pool;
use user::println;

struct Workers(Pool);

impl Parallel for Workers {
    fn workers(&self) -> usize {
        self.0.workers()
    }

    fn broadcast(&self, job: &(dyn Fn(usize, usize) + Sync)) {
        self.0.broadcast(job);
    }
}

/// A pool of up to `kabi::thread::MAX_WORKERS` workers, or the service's own
/// thread alone when the kernel starts none.
pub fn start() -> &'static dyn Parallel {
    match Pool::new(kabi::thread::MAX_WORKERS) {
        Ok(pool) => Box::leak(Box::new(Workers(pool))),
        Err(error) => {
            println!("local: no worker threads ({error:?}); the engine runs on one");
            &Serial
        }
    }
}
