//! A persistent thread pool on the host, close to the guest's
//! `user::pool::Pool`: workers started once, a generation counter they watch, and
//! a short spin before they park. Unlike the guest's, its workers are not pinned
//! and its caller spins briefly before it parks (the guest's sleeps at once), so
//! its hand-off is somewhat cheaper than Kernel AI's will be.

use quant::Parallel;
use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle, Thread};

/// Polls of the generation (or of the finished count) before parking, as the
/// guest pool spins before FUTEX_WAIT.
const SPINS: u32 = 2_000;

/// A job with its lifetime erased; see `Shared`.
type Job = dyn Fn(usize, usize) + Sync + 'static;

struct Shared {
    workers: usize,
    generation: AtomicU64,
    finished: AtomicUsize,
    stop: AtomicBool,
    /// A job panicked on a worker this generation (in test builds, which unwind).
    panicked: AtomicBool,
    /// The job of the current generation, with its lifetime erased.
    job: UnsafeCell<Option<*const Job>>,
    caller: UnsafeCell<Option<Thread>>,
}

// SAFETY: `job` and `caller` are written only by `broadcast` before it bumps the
// generation (Release) and read by workers only after they see the new
// generation (Acquire); `broadcast` takes `&self` of a pool that is not Sync, so
// two broadcasts never overlap, and it returns only once every worker has
// finished with them.
unsafe impl Sync for Shared {}
// SAFETY: as above; the raw job pointer is only dereferenced under that protocol.
unsafe impl Send for Shared {}

pub struct HostPool {
    shared: Arc<Shared>,
    threads: Vec<(Thread, JoinHandle<()>)>,
    /// Not Sync: one broadcast at a time, as the guest pool.
    _one_caller: std::marker::PhantomData<std::cell::Cell<()>>,
}

impl HostPool {
    pub fn new(workers: usize) -> Self {
        let shared = Arc::new(Shared {
            workers,
            generation: AtomicU64::new(0),
            finished: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            panicked: AtomicBool::new(false),
            job: UnsafeCell::new(None),
            caller: UnsafeCell::new(None),
        });
        let threads = (0..workers)
            .map(|index| {
                let shared = Arc::clone(&shared);
                let handle = thread::spawn(move || work(&shared, index));
                (handle.thread().clone(), handle)
            })
            .collect();
        Self {
            shared,
            threads,
            _one_caller: std::marker::PhantomData,
        }
    }
}

fn work(shared: &Shared, index: usize) {
    let mut seen = 0;
    loop {
        let mut spins = 0;
        while shared.generation.load(Ordering::Acquire) == seen {
            if spins < SPINS {
                spins += 1;
                std::hint::spin_loop();
            } else {
                thread::park();
            }
        }
        seen = shared.generation.load(Ordering::Acquire);
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        // SAFETY: the new generation was published after `job` was written, and
        // `broadcast` keeps the job alive until `finished` reaches `workers`.
        let job = unsafe { (*shared.job.get()).expect("a job for this generation") };
        // SAFETY: as above. The caller's handle is taken now too: once this
        // worker counts itself finished, the next broadcast may overwrite both.
        let caller = unsafe { (*shared.caller.get()).clone() };
        // A panicking job still counts as finished, so the caller wakes and
        // passes the panic on rather than waiting forever.
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: as above.
            unsafe { (*job)(index, shared.workers) }
        }));
        if ran.is_err() {
            shared.panicked.store(true, Ordering::Relaxed);
        }
        if shared.finished.fetch_add(1, Ordering::AcqRel) + 1 == shared.workers
            && let Some(caller) = caller
        {
            caller.unpark();
        }
    }
}

impl Parallel for HostPool {
    fn workers(&self) -> usize {
        self.shared.workers
    }

    fn broadcast(&self, job: &(dyn Fn(usize, usize) + Sync)) {
        let shared = &*self.shared;
        // SAFETY: the pointer is only used by workers of this generation, and
        // this call returns only after all of them have finished with it.
        let erased: *const Job =
            unsafe { std::mem::transmute::<&(dyn Fn(usize, usize) + Sync), &'static Job>(job) };
        // SAFETY: no worker reads these until the generation below changes; see
        // Shared.
        unsafe {
            *shared.job.get() = Some(erased);
            *shared.caller.get() = Some(thread::current());
        }
        shared.finished.store(0, Ordering::Relaxed);
        shared.generation.fetch_add(1, Ordering::Release);
        for (thread, _) in &self.threads {
            thread.unpark();
        }
        let mut spins = 0;
        while shared.finished.load(Ordering::Acquire) < shared.workers {
            if spins < SPINS {
                spins += 1;
                std::hint::spin_loop();
            } else {
                thread::park();
            }
        }
        if shared.panicked.swap(false, Ordering::Relaxed) {
            panic!("a job panicked on a pool worker");
        }
    }
}

impl Drop for HostPool {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.generation.fetch_add(1, Ordering::Release);
        for (thread, handle) in self.threads.drain(..) {
            thread.unpark();
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "panicked on a pool worker")]
    fn a_panicking_job_fails_the_caller() {
        let pool = HostPool::new(3);
        pool.broadcast(&|worker, _| assert_ne!(worker, 1));
    }

    #[test]
    fn every_worker_runs_each_broadcast_once() {
        for workers in [1, 3, 8] {
            let pool = HostPool::new(workers);
            for round in 0..200 {
                let hits = AtomicUsize::new(0);
                let mask = AtomicU64::new(0);
                pool.broadcast(&|worker, n| {
                    assert_eq!(n, workers);
                    hits.fetch_add(1, Ordering::Relaxed);
                    mask.fetch_or(1 << worker, Ordering::Relaxed);
                });
                assert_eq!(hits.load(Ordering::Relaxed), workers, "round {round}");
                assert_eq!(mask.load(Ordering::Relaxed), (1 << workers) - 1);
            }
        }
    }
}
