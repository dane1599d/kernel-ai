//! `mode=futex`, `mode=park` and `mode=spin`: futex wait and wake, and processes the
//! kernel kills mid-run.

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use kabi::Error;
use kabi::thread::MAX_WORKERS;
use user::{println, sys};

const STACK: u64 = 4;
/// Ping-pong rounds between main and each worker.
const ROUNDS: u32 = 500;

static TURN: AtomicU32 = AtomicU32::new(0);
static GATE: AtomicU32 = AtomicU32::new(0);
static IDLE: AtomicU32 = AtomicU32::new(0);
/// Ping-pong workers finished, gated workers about to wait, and gated workers past.
static DONE: AtomicUsize = AtomicUsize::new(0);
static ARRIVED: AtomicUsize = AtomicUsize::new(0);
static PASSED: AtomicUsize = AtomicUsize::new(0);

fn cpus() -> usize {
    sys::metrics()
        .map_or(1, |m| m.cpus as usize)
        .clamp(1, MAX_WORKERS)
}

/// Sleeps on `word` until it holds `want`.
fn wait_for(word: &AtomicU32, want: u32) -> Result<(), Error> {
    loop {
        let now = word.load(Ordering::Acquire);
        if now == want {
            return Ok(());
        }
        sys::futex_wait(word, now, 0)?;
    }
}

fn finish() -> ! {
    let refused = sys::thread_exit();
    println!("workers: THREAD_EXIT failed: {refused:?}");
    sys::exit(90)
}

/// The worker half of the ping-pong: answers each odd turn with the next even one.
extern "C" fn pong(_: u64) -> ! {
    for round in 0..ROUNDS {
        if wait_for(&TURN, 2 * round + 1).is_err() {
            sys::exit(91);
        }
        TURN.store(2 * round + 2, Ordering::Release);
        let _ = sys::futex_wake(&TURN, 1);
    }
    DONE.fetch_add(1, Ordering::AcqRel);
    finish()
}

/// Waits at the gate until main opens it.
extern "C" fn gated(_: u64) -> ! {
    ARRIVED.fetch_add(1, Ordering::AcqRel);
    if wait_for(&GATE, 1).is_err() {
        sys::exit(92);
    }
    PASSED.fetch_add(1, Ordering::AcqRel);
    finish()
}

/// Sleeps until `counter` reaches `count`, or fails after 20 s.
fn wait_count(counter: &AtomicUsize, count: usize) -> bool {
    let deadline = sys::time_ns() + 20_000_000_000;
    while counter.load(Ordering::Acquire) < count {
        if sys::time_ns() > deadline {
            return false;
        }
        sys::sleep_ms(1);
    }
    true
}

/// `mode=futex`: every check of the two calls; the first that fails gives the code.
pub fn futex() -> i32 {
    let cpus = cpus();
    // Ping-pong with a worker on every CPU in turn, through one word.
    for cpu in 0..cpus {
        TURN.store(0, Ordering::Release);
        if sys::thread_spawn(pong, 0, cpu, STACK).is_err() {
            return 100;
        }
        for round in 0..ROUNDS {
            TURN.store(2 * round + 1, Ordering::Release);
            if sys::futex_wake(&TURN, 1).is_err() || wait_for(&TURN, 2 * round + 2).is_err() {
                return 101;
            }
        }
        // The worker is done once it counts itself, before the next one starts.
        if !wait_count(&DONE, cpu + 1) {
            return 101;
        }
    }
    // A value that is not the one expected returns at once; no waiter, no wake.
    let word = AtomicU32::new(7);
    if sys::futex_wait(&word, 8, 0) != Ok(1) || sys::futex_wake(&word, 1) != Ok(0) {
        return 102;
    }
    // A timeout returns TimedOut, and not before it.
    let start = sys::time_ns();
    if sys::futex_wait(&word, 7, 20) != Err(Error::TimedOut) {
        return 103;
    }
    if sys::time_ns() - start < 20_000_000 {
        return 104;
    }
    // Misaligned is Invalid; unmapped user memory and kernel addresses are Faults.
    let misaligned = (&word as *const AtomicU32)
        .cast::<u8>()
        .wrapping_add(1)
        .cast();
    if sys::futex_wait(misaligned, 7, 0) != Err(Error::Invalid) {
        return 105;
    }
    let unmapped = (kabi::layout::HEAP_BASE + kabi::layout::HEAP_MAX - 4096) as *const AtomicU32;
    let kernel = 0xFFFF_8000_0000_0000_u64 as *const AtomicU32;
    if sys::futex_wait(unmapped, 0, 0) != Err(Error::Fault)
        || sys::futex_wait(kernel, 0, 0) != Err(Error::Fault)
    {
        return 106;
    }
    // A wake of every waiter: workers parked at a gate all pass it, and at least
    // one of them was really asleep there when it opened.
    for cpu in 0..cpus {
        if sys::thread_spawn(gated, 0, cpu, STACK).is_err() {
            return 107;
        }
    }
    if !wait_count(&ARRIVED, cpus) {
        return 107;
    }
    sys::sleep_ms(50);
    GATE.store(1, Ordering::Release);
    match sys::futex_wake(&GATE, u32::MAX) {
        Ok(woken) if woken >= 1 => {}
        other => {
            println!("workers: the gate's wake-all gave {other:?}");
            return 108;
        }
    }
    if !wait_count(&PASSED, cpus) {
        return 109;
    }
    println!("workers: futex checks passed; {ROUNDS} ping-pong rounds with each of {cpus} CPUs");
    0
}

extern "C" fn parked(_: u64) -> ! {
    let _ = wait_for(&IDLE, 1);
    sys::exit(93)
}

/// `mode=park`: a worker on every CPU and the main thread wait on futexes with no
/// timeout; only a kill ends the process.
pub fn park() -> i32 {
    for cpu in 0..cpus() {
        if sys::thread_spawn(parked, 0, cpu, STACK).is_err() {
            return 110;
        }
    }
    let _ = wait_for(&IDLE, 1);
    111
}

extern "C" fn compute(seed: u64) -> ! {
    // f32 work keeps vector registers live in every worker until the kill.
    let mut acc = [seed as f32; 8];
    let mut step = 0.0f32;
    loop {
        step += 1.0;
        for lane in acc.iter_mut() {
            *lane = core::hint::black_box(*lane * 0.999 + step);
        }
    }
}

/// `mode=spin`: a worker on every CPU computes in ring 3 forever while main waits
/// on a futex; only a kill ends the process.
pub fn spin() -> i32 {
    for cpu in 0..cpus() {
        if sys::thread_spawn(compute, cpu as u64, cpu, STACK).is_err() {
            return 120;
        }
    }
    let _ = wait_for(&IDLE, 1);
    121
}
