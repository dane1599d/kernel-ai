//! `test=icount-probe` (M10a-2): is a boot under QEMU's icount deterministic once
//! the kernel runs, and how much host time does a virtual second cost?
//!
//! The case starts three kernel threads and lets each block, so their stacks are
//! allocated before anything is measured. It then waits for a tick edge and, printing
//! nothing, wakes the threads, which alternate seeded spins with seeded sleeps,
//! while it runs a seeded compute loop for one virtual second. Every time it
//! records is relative to that tick-aligned start (tick counts and HPET
//! nanoseconds), never absolute: firmware and boot-disk I/O before the kernel are
//! host-timed, so the virtual time at kernel entry may differ from boot to boot.
//! Afterwards it prints an FNV-1a digest of the records and of the work done;
//! xtask boots it twice and requires the same digest. The `measure-start` and
//! `measure-end` lines bracket a span whose virtual length is printed after them,
//! so xtask can divide the host time it took by the virtual time.

use crate::{hpet, log, threads, timer};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use kabi::model::Fnv1a;

const SLEEPERS: usize = 3;
const ROUNDS: usize = 20;
/// Virtual time the compute loop runs for.
const COMPUTE_NS: u64 = 1_000_000_000;
/// Compute iterations between clock reads.
const CHUNK: u64 = 4096;
const DEFAULT_SEED: u64 = 1;
const WAIT_TIMEOUT_MS: u64 = 30_000;

/// Per sleeper and round: ticks and HPET nanoseconds since the aligned start.
static RECORDS: [[[AtomicU64; 2]; ROUNDS]; SLEEPERS] =
    [const { [const { [const { AtomicU64::new(0) }; 2] }; ROUNDS] }; SLEEPERS];
static START_TICK: AtomicU64 = AtomicU64::new(0);
static START_NS: AtomicU64 = AtomicU64::new(0);
static SEED: AtomicU64 = AtomicU64::new(DEFAULT_SEED);
static STARTED: AtomicBool = AtomicBool::new(false);
static READY: AtomicUsize = AtomicUsize::new(0);
static FINISHED: AtomicUsize = AtomicUsize::new(0);

/// xorshift64*: the probe's only source of variety, so a seed fixes all of it.
fn next(state: &mut u64) -> u64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// FNV-1a over the little-endian bytes of each value.
fn digest(values: impl IntoIterator<Item = u64>) -> u64 {
    let mut hash = Fnv1a::new();
    for value in values {
        hash.update(&value.to_le_bytes());
    }
    hash.finish()
}

/// Ticks and nanoseconds since the aligned start.
fn since_start() -> (u64, u64) {
    (
        timer::ticks() - START_TICK.load(Ordering::SeqCst),
        hpet::nanos() - START_NS.load(Ordering::SeqCst),
    )
}

fn sleeper(index: usize) {
    READY.fetch_add(1, Ordering::SeqCst);
    while !STARTED.load(Ordering::SeqCst) {
        threads::block_unless(|| STARTED.load(Ordering::SeqCst));
    }
    let mut state = (SEED.load(Ordering::SeqCst) ^ ((index as u64 + 1) << 32)) | 1;
    for round in &RECORDS[index] {
        let mut sink = 0u64;
        for _ in 0..next(&mut state) % 50_000 {
            sink = sink.wrapping_add(next(&mut state));
        }
        core::hint::black_box(sink);
        threads::sleep_ms(1 + next(&mut state) % 7);
        let (ticks, nanos) = since_start();
        round[0].store(ticks, Ordering::SeqCst);
        round[1].store(nanos, Ordering::SeqCst);
    }
    FINISHED.fetch_add(1, Ordering::SeqCst);
}

/// The seed from `seed=N` on the command line, or 1.
fn seed_from(cmdline: &str) -> u64 {
    cmdline
        .split_whitespace()
        .find_map(|arg| arg.strip_prefix("seed=")?.parse().ok())
        .unwrap_or(DEFAULT_SEED)
}

/// Sleeps in 10 ms steps until `done()`, or fails after 30 s.
fn wait_until(done: impl Fn() -> bool, failure: &'static str) -> Result<(), &'static str> {
    let mut waited = 0;
    while !done() {
        if waited >= WAIT_TIMEOUT_MS {
            return Err(failure);
        }
        threads::sleep_ms(10);
        waited += 10;
    }
    Ok(())
}

pub fn check(cmdline: &str) -> Result<(), &'static str> {
    let seed = seed_from(cmdline);
    SEED.store(seed, Ordering::SeqCst);
    let mut tids = [0; SLEEPERS];
    for (index, tid) in tids.iter_mut().enumerate() {
        *tid = threads::spawn("icount-sleeper", sleeper, index)?;
    }
    wait_until(
        || READY.load(Ordering::SeqCst) == SLEEPERS,
        "icount-probe: the sleeper threads never started",
    )?;
    let marked = hpet::nanos();
    log!("icount-probe: seed {seed}; measure-start");
    // Start on a tick edge: the first instruction after a timer interrupt.
    let edge = timer::ticks();
    while timer::ticks() == edge {
        x86_64::instructions::interrupts::enable_and_hlt();
    }
    START_TICK.store(timer::ticks(), Ordering::SeqCst);
    START_NS.store(hpet::nanos(), Ordering::SeqCst);
    STARTED.store(true, Ordering::SeqCst);
    for tid in tids {
        x86_64::instructions::interrupts::without_interrupts(|| threads::wake(tid));
    }
    let (mut state, mut iterations, mut mix) = (seed | 1, 0u64, 0u64);
    while since_start().1 < COMPUTE_NS {
        for _ in 0..CHUNK {
            mix = mix.rotate_left(7) ^ next(&mut state);
        }
        iterations += CHUNK;
    }
    let computed = since_start();
    wait_until(
        || FINISHED.load(Ordering::SeqCst) == SLEEPERS,
        "icount-probe: the sleeper threads did not finish",
    )?;
    let end = since_start();
    let span_ns = hpet::nanos() - marked;
    log!("icount-probe: measure-end");
    let sleepers: [u64; SLEEPERS] = core::array::from_fn(|index| {
        digest(
            RECORDS[index]
                .iter()
                .flat_map(|round| round.iter().map(|value| value.load(Ordering::SeqCst))),
        )
    });
    let compute = digest([iterations, mix, computed.0, computed.1]);
    let all = digest(
        [compute, end.0, end.1]
            .into_iter()
            .chain(sleepers.iter().copied()),
    );
    log!(
        "icount-probe: aligned at tick {} and HPET {} ns (absolute, not digested)",
        START_TICK.load(Ordering::SeqCst),
        START_NS.load(Ordering::SeqCst)
    );
    log!(
        "icount-probe: compute {compute:016x} ({iterations} iterations), sleepers {:016x} {:016x} {:016x}",
        sleepers[0],
        sleepers[1],
        sleepers[2]
    );
    log!("icount-probe: digest {all:016x}");
    log!(
        "icount-probe: {} ticks in the window; {} us of virtual time between the markers",
        end.0,
        span_ns / 1000
    );
    Ok(())
}
