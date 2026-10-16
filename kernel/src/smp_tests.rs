//! M5 self-tests: every CPU runs threads, TLB shootdown makes a remapped page visible
//! on all CPUs, and a spawn/exit/sleep storm neither deadlocks nor loses a thread.

use crate::memory::{self, SCRATCH_START};
use crate::percpu::{self, MAX_CPUS};
use crate::{hpet, log, smp, threads};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use x86_64::structures::paging::PageTableFlags;

const RW: PageTableFlags = PageTableFlags::PRESENT
    .union(PageTableFlags::WRITABLE)
    .union(PageTableFlags::NO_EXECUTE);

/// Waits on the calling thread, sleeping between checks, until `done()` or timeout.
fn wait_for(done: impl Fn() -> bool, timeout_ms: u64) -> bool {
    for _ in 0..timeout_ms / 5 {
        if done() {
            return true;
        }
        threads::sleep_ms(5);
    }
    done()
}

static WORK_ON_CPU: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static PIN_SAMPLES: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static PIN_VIOLATIONS: AtomicU64 = AtomicU64::new(0);
static STOP_PINNED: AtomicBool = AtomicBool::new(false);
/// Iterations a pinned reporter spins between samples, so it is often preempted and
/// sits runnable where the load balancer could see it.
const REPORTER_SPIN: u32 = 20_000;

fn worker(_: usize) {
    loop {
        WORK_ON_CPU[percpu::index()].fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
}

/// Samples its CPU for the whole test and counts every sample taken elsewhere.
fn pinned_reporter(expected_cpu: usize) {
    while !STOP_PINNED.load(Ordering::Acquire) {
        if percpu::index() != expected_cpu {
            PIN_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
        }
        PIN_SAMPLES[expected_cpu].fetch_add(1, Ordering::Relaxed);
        for _ in 0..REPORTER_SPIN {
            core::hint::spin_loop();
        }
    }
}

/// Extra load pinned to CPU 0, so the balancer keeps trying to pull work off it.
/// It also checks that it stays on its CPU.
fn pinned_hog(expected_cpu: usize) {
    while !STOP_PINNED.load(Ordering::Acquire) {
        if percpu::index() != expected_cpu {
            PIN_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
        }
        for _ in 0..REPORTER_SPIN {
            core::hint::spin_loop();
        }
    }
}

/// Boot check: every CPU Limine reported is online.
pub fn boot_check(expected: usize) -> Result<(), &'static str> {
    let online = smp::online();
    log!("self-test: {online} of {expected} CPUs online");
    if online == expected {
        Ok(())
    } else {
        Err("smp: not every CPU came online")
    }
}

/// M5 exit test: unpinned threads spread to every CPU, the balancer moves work off an
/// overloaded CPU, and pinned threads never leave their CPU while that happens.
pub fn all_cpus_check() -> Result<(), &'static str> {
    const CPU0_EXTRA_HOGS: usize = 3;
    let cpus = smp::online();
    // Counted from the start: the balancer may act as soon as the work exists.
    let migrations_before = threads::migrations();
    for _ in 0..2 * cpus {
        threads::spawn("worker", worker, 0)?;
    }
    for cpu in 0..cpus {
        threads::spawn_pinned(cpu, "pinned-reporter", pinned_reporter, cpu)?;
    }
    if cpus > 1 {
        for _ in 0..CPU0_EXTRA_HOGS {
            threads::spawn_pinned(0, "pinned-hog", pinned_hog, 0)?;
        }
    }
    threads::sleep_ms(1_000);
    STOP_PINNED.store(true, Ordering::Release);
    let migrated = threads::migrations() - migrations_before;
    let work: [u64; MAX_CPUS] = core::array::from_fn(|i| WORK_ON_CPU[i].load(Ordering::Relaxed));
    let samples: [u64; MAX_CPUS] = core::array::from_fn(|i| PIN_SAMPLES[i].load(Ordering::Relaxed));
    let violations = PIN_VIOLATIONS.load(Ordering::SeqCst);
    log!(
        "smp: worker progress per CPU {:?}; pinned samples {:?}, {violations} on the wrong CPU; {migrated} migrations",
        &work[..cpus],
        &samples[..cpus]
    );
    if work[..cpus].contains(&0) {
        return Err("smp: a CPU never ran a worker thread");
    }
    if samples[..cpus].contains(&0) {
        return Err("smp: a pinned reporter never ran");
    }
    if violations > 0 {
        return Err("smp: a pinned thread ran on the wrong CPU");
    }
    if cpus > 1 && migrated == 0 {
        return Err("smp: the load balancer never moved work off the overloaded CPU");
    }
    Ok(())
}

const TLB_PAGE_A: u64 = SCRATCH_START + 0x40_0000;
const TLB_PAGE_B: u64 = SCRATCH_START + 0x40_1000;
const OLD_VALUE: u64 = 0x1111_1111;
const NEW_VALUE: u64 = 0x2222_2222;

static TLB_ROUND: AtomicUsize = AtomicUsize::new(0);
static TLB_READS: AtomicUsize = AtomicUsize::new(0);
static TLB_SEEN: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Reads page A once per round. Pinned to one CPU, so its TLB keeps the mapping.
fn tlb_reader(_: usize) {
    let mut done_round = 0;
    while done_round < 2 {
        if TLB_ROUND.load(Ordering::Acquire) > done_round {
            done_round += 1;
            // SAFETY: page A is mapped for the whole test.
            let value = unsafe { (TLB_PAGE_A as *const u64).read_volatile() };
            TLB_SEEN[percpu::index()].store(value, Ordering::Release);
            TLB_READS.fetch_add(1, Ordering::AcqRel);
        } else {
            threads::sleep_ms(1);
        }
    }
}

/// M5 exit test: every CPU caches page A's mapping, A is retargeted to B's frame,
/// and every CPU must then read B's value. Without shootdown, stale TLB entries
/// would still show A's old value.
pub fn tlb_check() -> Result<(), &'static str> {
    let cpus = smp::online();
    memory::with(|m| {
        m.map(TLB_PAGE_A, RW)?;
        m.map(TLB_PAGE_B, RW)
    })?;
    // SAFETY: both pages were just mapped writable.
    unsafe {
        (TLB_PAGE_A as *mut u64).write_volatile(OLD_VALUE);
        (TLB_PAGE_B as *mut u64).write_volatile(NEW_VALUE);
    }
    for cpu in 0..cpus {
        threads::spawn_pinned(cpu, "tlb-reader", tlb_reader, 0)?;
    }
    for (round, expected) in [(1, OLD_VALUE), (2, NEW_VALUE)] {
        if round == 2 {
            let frame_b = memory::with(|m| m.translate(TLB_PAGE_B))
                .ok_or("page B vanished")?
                .0;
            let old_frame = memory::with(|m| m.retarget(TLB_PAGE_A, frame_b, RW))?;
            memory::with(|m| m.free_frame(old_frame))?;
        }
        TLB_READS.store(0, Ordering::Release);
        TLB_ROUND.store(round, Ordering::Release);
        if !wait_for(|| TLB_READS.load(Ordering::Acquire) == cpus, 2_000) {
            return Err("smp: TLB readers did not all report");
        }
        let seen: [u64; MAX_CPUS] = core::array::from_fn(|i| TLB_SEEN[i].load(Ordering::Acquire));
        if seen[..cpus].iter().any(|&value| value != expected) {
            log!(
                "smp: round {round} expected {expected:#x}, CPUs saw {:x?}",
                &seen[..cpus]
            );
            return Err("smp: a CPU read through a stale TLB entry");
        }
    }
    memory::with(|m| {
        m.unmap_keep_frame(TLB_PAGE_A)?;
        m.unmap(TLB_PAGE_B)
    })?;
    log!("smp: TLB shootdown made the remapped page visible on all {cpus} CPUs");
    Ok(())
}

static STORM_END_NS: AtomicU64 = AtomicU64::new(0);
static STORM_SPAWNED: AtomicU64 = AtomicU64::new(0);
static STORM_FINISHED: AtomicU64 = AtomicU64::new(0);
static STORM_FULL: AtomicU64 = AtomicU64::new(0);

fn storm_child(seed: usize) {
    threads::sleep_ms((seed % 4) as u64);
    let mut x = seed as u64;
    for _ in 0..(seed % 2000) {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
    }
    core::hint::black_box(x);
    STORM_FINISHED.fetch_add(1, Ordering::AcqRel);
}

fn stormer(seed: usize) {
    let mut state = seed as u64 | 1;
    while hpet::nanos() < STORM_END_NS.load(Ordering::Acquire) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        match threads::spawn("storm-child", storm_child, state as usize) {
            Ok(_) => {
                STORM_SPAWNED.fetch_add(1, Ordering::AcqRel);
            }
            Err(_) => {
                STORM_FULL.fetch_add(1, Ordering::Relaxed);
                threads::sleep_ms(1);
            }
        }
        if state.is_multiple_of(3) {
            threads::sleep_ms(state % 3);
        }
    }
}

/// M5 exit test: four threads spawn short-lived children for `seconds`. Progress must
/// continue the whole time (no deadlock), and every spawned child must finish.
pub fn storm_check(seconds: u64) -> Result<(), &'static str> {
    const STORMERS: usize = 4;
    STORM_END_NS.store(hpet::nanos() + seconds * 1_000_000_000, Ordering::Release);
    log!(
        "storm: measure-start, {STORMERS} spawners for {seconds} s on {} CPUs",
        smp::online()
    );
    for i in 0..STORMERS {
        threads::spawn("stormer", stormer, 0x9E37_79B9 + i * 7919)?;
    }
    let end = STORM_END_NS.load(Ordering::Acquire);
    let mut last = 0;
    for second in 1..=seconds {
        threads::sleep_ms(1_000);
        let spawned = STORM_SPAWNED.load(Ordering::Acquire);
        // Each one-second sleep runs slightly long, so late windows can reach past
        // the deadline, where spawning stops by design. Only judge whole windows.
        if hpet::nanos() >= end {
            break;
        }
        if spawned == last {
            log!("storm: no new threads in second {second}");
            return Err("storm: spawning stopped making progress");
        }
        if second % 10 == 0 || second == seconds {
            log!(
                "storm: {second} s, {spawned} spawned, {} finished",
                STORM_FINISHED.load(Ordering::Acquire)
            );
        }
        last = spawned;
    }
    let drained = wait_for(
        || STORM_FINISHED.load(Ordering::Acquire) == STORM_SPAWNED.load(Ordering::Acquire),
        10_000,
    );
    let (spawned, finished) = (
        STORM_SPAWNED.load(Ordering::Acquire),
        STORM_FINISHED.load(Ordering::Acquire),
    );
    let (_, _, switches) = threads::stats();
    log!(
        "storm: measure-end, {spawned} spawned, {finished} finished, table full {} times, {switches} switches",
        STORM_FULL.load(Ordering::Relaxed)
    );
    if !drained {
        return Err("storm: some spawned threads never finished");
    }
    Ok(())
}
