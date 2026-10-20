//! L6 self-tests for the engine's thread pool, with `workers` processes the kernel
//! starts itself.
//!
//! - `test=matvec`: an f32 matrix-vector product split by rows over a pool with a
//!   worker pinned on every CPU gives the same bits as one thread, for 50 rounds.
//! - `test=compute-keys`: while ring-3 workers compute on every CPU, the harness
//!   types `kai` and Enter; the keyboard thread, pinned to CPU 0 where IRQ1 lands,
//!   must run within 20 ms of every key's interrupt, and the workers must still be
//!   computing at the end. Their process has no burst limit; the test kills it.

use crate::fpu_tests::wait_exit;
use crate::memory::user::USER_FRAMES;
use crate::thread_tests::{settle, start};
use crate::{hpet, log, process, sched_tests, smp, threads};
use core::sync::atomic::Ordering;
use kabi::{ExitReason, cap};

/// The keyboard thread must run this soon after each key's interrupt.
const LATENCY_LIMIT_NS: u64 = 20_000_000;
/// How long the workers may take to start.
const START_MS: u64 = 10_000;

/// The `matvec` case.
pub fn matvec_check() -> Result<(), &'static str> {
    let slots = threads::in_use();
    let pid = start("matvec", "mode=matvec", cap::THREADS, 10_000)?;
    match wait_exit(pid)? {
        (ExitReason::Exited, 0) => {}
        (reason, detail) => {
            log!("matvec: ended {} ({detail})", reason.name());
            return Err("matvec: the parallel result differed or the pool failed");
        }
    }
    settle(slots)?;
    if USER_FRAMES.load(Ordering::SeqCst) != 0 {
        return Err("matvec: user frames leaked");
    }
    log!(
        "matvec: the pool's result matched one thread's on {} CPUs",
        smp::online()
    );
    Ok(())
}

/// The `compute-keys` case.
pub fn keys_check() -> Result<(), &'static str> {
    let cpus = smp::online();
    let pid = start("compute", "mode=spin", cap::THREADS, 0)?;
    // Every worker registered, then a moment so all of them are computing.
    let deadline = hpet::nanos() + START_MS * 1_000_000;
    while process::thread_count(pid) < cpus + 1 {
        if hpet::nanos() > deadline {
            return Err("compute-keys: the workers did not start");
        }
        threads::sleep_ms(10);
    }
    threads::sleep_ms(200);
    log!("compute-keys: {cpus} workers computing in ring 3, one per CPU");
    let (wakes, worst) = sched_tests::typed_line(Some(0), "compute-keys")?;
    let still = process::thread_count(pid);
    log!(
        "compute-keys: {wakes} keyboard wake-ups, worst latency {} us (limit {} us); {still} threads still running",
        worst / 1000,
        LATENCY_LIMIT_NS / 1000
    );
    process::kill(pid, ExitReason::Killed).map_err(|_| "compute-keys: the workers had ended")?;
    if wait_exit(pid)?.0 != ExitReason::Killed {
        return Err("compute-keys: the workers' process ended the wrong way");
    }
    if still != cpus + 1 {
        return Err("compute-keys: a worker stopped computing");
    }
    if wakes == 0 || worst >= LATENCY_LIMIT_NS {
        return Err("compute-keys: a key took 20 ms or more to reach its thread");
    }
    Ok(())
}
