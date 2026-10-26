//! `mode=basic`, `mode=overflow` and `mode=nocap`: starting and ending workers.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use kabi::Error;
use kabi::thread::{MAX_STACK_PAGES, MAX_WORKERS};
use user::{println, sys};

/// Stack pages for the workers here: they run small functions.
const STACK: u64 = 4;
/// Longest any wait for workers takes.
const WAIT_MS: u64 = 20_000;

static DONE: AtomicUsize = AtomicUsize::new(0);
static RESULTS: [AtomicU64; MAX_WORKERS] = [const { AtomicU64::new(0) }; MAX_WORKERS];
static RELEASE: AtomicBool = AtomicBool::new(false);
/// What a worker's SVC_RECV and ARGS returned, as raw results.
static FILTERED: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

fn mix(x: u64) -> u64 {
    let x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^ (x >> 27)
}

fn cpus() -> usize {
    sys::metrics()
        .map_or(1, |m| m.cpus as usize)
        .clamp(1, MAX_WORKERS)
}

/// Ends the calling worker; a refusal is a failed check.
fn finish() -> ! {
    let refused = sys::thread_exit();
    println!("workers: THREAD_EXIT from a worker failed: {refused:?}");
    sys::exit(90)
}

/// THREAD_SPAWN, retried while it says Busy: a worker counts itself done just
/// before THREAD_EXIT, and its index is free only once the kernel has let it go.
fn spawn(entry: extern "C" fn(u64) -> !, arg: u64, cpu: usize) -> Result<usize, Error> {
    let deadline = sys::time_ns() + WAIT_MS * 1_000_000;
    loop {
        match sys::thread_spawn(entry, arg, cpu, STACK) {
            Err(Error::Busy) if sys::time_ns() < deadline => sys::sleep_ms(1),
            other => return other,
        }
    }
}

/// Waits until `DONE` reaches `count`, sleeping between looks.
fn wait_done(count: usize) -> bool {
    let deadline = sys::time_ns() + WAIT_MS * 1_000_000;
    while DONE.load(Ordering::Acquire) < count {
        if sys::time_ns() > deadline {
            return false;
        }
        sys::sleep_ms(1);
    }
    true
}

extern "C" fn compute(index: u64) -> ! {
    RESULTS[index as usize].store(mix(index), Ordering::Release);
    DONE.fetch_add(1, Ordering::AcqRel);
    finish()
}

extern "C" fn park(_: u64) -> ! {
    while !RELEASE.load(Ordering::Acquire) {
        sys::sleep_ms(1);
    }
    DONE.fetch_add(1, Ordering::AcqRel);
    finish()
}

/// Sleeps an hour at a time: only a wake that a kill or the process's end sends
/// can end it within the test's limits.
extern "C" fn forever(_: u64) -> ! {
    loop {
        sys::sleep_ms(3_600_000);
    }
}

/// Calls that main may make and a worker may not: only the filter refuses them.
extern "C" fn filtered(_: u64) -> ! {
    let mut buf = [0u8; 8];
    let mut caller = 0;
    let recv = sys::svc_recv(&mut buf, 1, &mut caller);
    let args = sys::args(&mut buf);
    let raw = |result: Result<usize, Error>| match result {
        Ok(value) => value as u64,
        Err(error) => error.to_raw(),
    };
    FILTERED[0].store(raw(recv), Ordering::Release);
    FILTERED[1].store(raw(args), Ordering::Release);
    DONE.fetch_add(1, Ordering::AcqRel);
    finish()
}

/// Every check of `mode=basic`; the first that fails gives the exit code.
pub fn run() -> i32 {
    let cpus = cpus();
    // A worker pinned on every CPU computes its value and exits.
    for cpu in 0..cpus {
        if let Err(error) = sys::thread_spawn(compute, cpu as u64, cpu, STACK) {
            println!("workers: THREAD_SPAWN on cpu {cpu}: {error:?}");
            return 10;
        }
    }
    if !wait_done(cpus) {
        return 11;
    }
    if (0..cpus).any(|i| RESULTS[i].load(Ordering::Acquire) != mix(i as u64)) {
        return 12;
    }
    // Refused: a CPU that is not online, stacks of no pages and of too many, an
    // entry point below user memory, and the main thread's THREAD_EXIT.
    let online = sys::metrics().map_or(cpus, |m| m.cpus as usize);
    let bad_cpu = sys::thread_spawn(forever, 0, online, STACK);
    let no_stack = sys::thread_spawn(forever, 0, 0, 0);
    let big_stack = sys::thread_spawn(forever, 0, 0, MAX_STACK_PAGES + 1);
    // SAFETY: the pointer is never called here; the kernel must refuse it.
    let low: extern "C" fn(u64) -> ! = unsafe { core::mem::transmute(0x8_usize) };
    let low_entry = sys::thread_spawn(low, 0, 0, STACK);
    if [bad_cpu, no_stack, big_stack, low_entry]
        .iter()
        .any(|result| *result != Err(Error::Invalid))
    {
        println!("workers: bad spawns gave {bad_cpu:?} {no_stack:?} {big_stack:?} {low_entry:?}");
        return 13;
    }
    if sys::thread_exit() != Error::Invalid {
        return 14;
    }
    // The call filter: main may wait for a request (none comes) and read its
    // arguments; a worker may do neither.
    let mut buf = [0u8; 8];
    let mut caller = 0;
    let recv = sys::svc_recv(&mut buf, 1, &mut caller);
    if recv != Err(Error::TimedOut) || sys::args(&mut buf).is_err() {
        return 15;
    }
    let base = DONE.load(Ordering::Acquire);
    if spawn(filtered, 0, 0).is_err() || !wait_done(base + 1) {
        return 16;
    }
    let permission = Error::Permission.to_raw();
    if FILTERED
        .iter()
        .any(|raw| raw.load(Ordering::Acquire) != permission)
    {
        println!("workers: a worker's SVC_RECV or ARGS was not refused");
        return 17;
    }
    // Every index taken, one more is Busy; released, they exit on their own.
    let base = DONE.load(Ordering::Acquire);
    for index in 0..MAX_WORKERS {
        if spawn(park, 0, index % cpus).is_err() {
            return 18;
        }
    }
    if sys::thread_spawn(park, 0, 0, STACK) != Err(Error::Busy) {
        return 19;
    }
    RELEASE.store(true, Ordering::Release);
    if !wait_done(base + MAX_WORKERS) {
        return 20;
    }
    // Workers that never end on their own: `exit` from main ends them too, and the
    // last one out reports this exit.
    for cpu in 0..cpus {
        if spawn(forever, 0, cpu).is_err() {
            return 21;
        }
    }
    sys::sleep_ms(20);
    println!("workers: basic checks passed on {cpus} CPUs; exiting with workers running");
    0
}

#[inline(never)]
fn deep(depth: u64) -> u64 {
    let frame = [depth; 64];
    let below = if depth == 0 { 0 } else { deep(depth - 1) };
    core::hint::black_box(&frame)[(depth % 64) as usize].wrapping_add(below)
}

extern "C" fn overflow_worker(_: u64) -> ! {
    // Far deeper than one page of stack: it must hit the unmapped guard.
    let sum = deep(1 << 20);
    println!("workers: the stack never overflowed ({sum})");
    sys::exit(31)
}

/// `mode=overflow`: the process must die of the worker's page fault.
pub fn overflow() -> i32 {
    if sys::thread_spawn(overflow_worker, 0, 0, 1).is_err() {
        return 30;
    }
    let deadline = sys::time_ns() + WAIT_MS * 1_000_000;
    while sys::time_ns() < deadline {
        sys::sleep_ms(10);
    }
    32
}

/// `mode=spawnexit`: main exits right after starting workers, maybe before any
/// of them ran; they must still be counted, killed and torn down with it.
pub fn spawn_exit() -> i32 {
    for cpu in 0..cpus() {
        if sys::thread_spawn(forever, 0, cpu, STACK).is_err() {
            return 50;
        }
    }
    0
}

/// `mode=sleep`: a worker on every CPU and main sleep an hour at a time; only the
/// kernel's kill, waking every thread, ends the process.
pub fn sleep() -> i32 {
    for cpu in 0..cpus() {
        if spawn(forever, 0, cpu).is_err() {
            return 55;
        }
    }
    loop {
        sys::sleep_ms(3_600_000);
    }
}

/// `mode=nocap`: THREAD_SPAWN without the capability.
pub fn nocap() -> i32 {
    match sys::thread_spawn(forever, 0, 0, STACK) {
        Err(Error::Permission) => {}
        other => {
            println!("workers: THREAD_SPAWN without cap::THREADS gave {other:?}");
            return 40;
        }
    }
    match sys::thread_pin(Some(0)) {
        Err(Error::Permission) => 0,
        other => {
            println!("workers: THREAD_PIN without cap::THREADS gave {other:?}");
            41
        }
    }
}

/// THREAD_PIN from ring 3: the main thread pins to the last CPU and back to CPU
/// 0, a CPU past the online count is Invalid, and unpinning works.
pub fn pin() -> i32 {
    let last = cpus() - 1;
    for (cpu, want, code) in [
        (Some(last), Ok(()), 50),
        (Some(0), Ok(()), 51),
        (Some(cpus() + 3), Err(Error::Invalid), 52),
        (None, Ok(()), 53),
    ] {
        let got = sys::thread_pin(cpu);
        if got != want {
            println!("workers: THREAD_PIN {cpu:?} gave {got:?}, wanted {want:?}");
            return code;
        }
    }
    0
}
