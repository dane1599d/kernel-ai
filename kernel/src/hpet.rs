//! HPET main counter: the kernel's reference clock.
//!
//! With `clock=tsc` on the command line, `nanos` reads the TSC instead, scaled by
//! a rate measured against the HPET at boot. Every timer tick reads the clock a
//! few times; under VirtualBox on Hyper-V each HPET read is a trip to VirtualBox's
//! device emulation, and with 8 vCPUs ticking at 1 kHz those trips take all the
//! CPU time there is. `cargo xtask vbox` boots with `clock=tsc`; QEMU keeps the
//! HPET.
//!
//! With `clock=icount`, `nanos` reads the TSC at exactly 1 GHz, unmeasured: under
//! QEMU's `-icount` the TSC is the virtual clock in nanoseconds, a fixed number per
//! instruction. The icount boots use it because the HPET's 10 ns counts do not
//! line up with icount's 8 ns steps, so what an HPET read returns depends on the
//! absolute virtual time, which the host-timed firmware and boot-drive reads set
//! before the kernel starts: the M10 observe boots reached their aligned start at
//! the same HPET time but different TSCs, and built different summaries.

use crate::percpu::{self, MAX_CPUS};
use crate::{log, memory};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use x86_64::structures::paging::PageTableFlags;

const CAPABILITIES: u64 = 0x000;
const CONFIGURATION: u64 = 0x010;
const MAIN_COUNTER: u64 = 0x0F0;
const ENABLE: u64 = 1;
const FEMTOS_PER_NANO: u128 = 1_000_000;
/// The HPET spec caps the tick period at 100 ns.
const MAX_PERIOD_FS: u64 = 100_000_000;

static BASE: AtomicU64 = AtomicU64::new(0);
static PERIOD_FS: AtomicU64 = AtomicU64::new(0);
/// Nanoseconds per TSC tick, as a 32.32 fixed-point number; 0 while the HPET is
/// the clock.
static TSC_MULT: AtomicU64 = AtomicU64::new(0);
/// Each CPU's TSC and the HPET's nanoseconds at one moment: VirtualBox starts a
/// vCPU's TSC when the vCPU first runs, so every CPU's TSC has its own zero.
static TSC_ZERO: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static NANOS_ZERO: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// The latest time any CPU returned: an HPET read takes microseconds, so two CPUs'
/// zero points can disagree by that much, and no caller may see time go back.
static LATEST: AtomicU64 = AtomicU64::new(0);
/// `clock=icount`: the TSC is the clock at exactly 1 GHz.
static ICOUNT: AtomicBool = AtomicBool::new(false);
const NANOS_PER_SECOND: u64 = 1_000_000_000;
/// One window of measuring the TSC against the HPET, and at most how many.
const WINDOW_NS: u64 = 25_000_000;
const WINDOWS: u32 = 200;
/// TSC ticks spun between two HPET reads while calibrating.
const SPIN_TICKS: u64 = 1 << 20;

/// The last main-counter value read without tearing, by any CPU.
static LAST_TICKS: AtomicU64 = AtomicU64::new(0);
/// Main-counter reads that were not near the last good value, so the halves were
/// read instead: torn reads, which QEMU before 10.0 gives at each 43 s wrap of
/// the low half.
static SPLIT_READS: AtomicU64 = AtomicU64::new(0);

fn read(offset: u64) -> u64 {
    // SAFETY: BASE maps the HPET register block uncached.
    unsafe { ((BASE.load(Ordering::Relaxed) + offset) as *const u64).read_volatile() }
}

fn read32(offset: u64) -> u32 {
    // SAFETY: as in read; the register block allows 32-bit reads.
    unsafe { ((BASE.load(Ordering::Relaxed) + offset) as *const u32).read_volatile() }
}

/// The main counter, never torn (see [`acpitables::hpet_counter`]). A torn read
/// once put a sleeping thread 43 s in the future and lost it from the storm case.
fn main_counter() -> u64 {
    let last = LAST_TICKS.load(Ordering::Relaxed);
    let (ticks, split) = acpitables::hpet_counter::read(
        || read(MAIN_COUNTER),
        || read32(MAIN_COUNTER),
        || read32(MAIN_COUNTER + 4),
        last,
    );
    if split {
        SPLIT_READS.fetch_add(1, Ordering::Relaxed);
    }
    LAST_TICKS.fetch_max(ticks, Ordering::Relaxed);
    ticks
}

/// Main-counter reads so far that had to fall back to reading the halves.
pub fn split_reads() -> u64 {
    SPLIT_READS.load(Ordering::Relaxed)
}

fn write(offset: u64, value: u64) {
    // SAFETY: as in read.
    unsafe { ((BASE.load(Ordering::Relaxed) + offset) as *mut u64).write_volatile(value) }
}

pub fn init(phys: u64) -> Result<(), &'static str> {
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::WRITE_THROUGH
        | PageTableFlags::NO_EXECUTE;
    BASE.store(memory::map_physical(phys, 0x400, flags)?, Ordering::Relaxed);
    let period = read(CAPABILITIES) >> 32;
    if period == 0 || period > MAX_PERIOD_FS {
        return Err("HPET: invalid counter period");
    }
    PERIOD_FS.store(period, Ordering::Relaxed);
    write(CONFIGURATION, read(CONFIGURATION) | ENABLE);
    Ok(())
}

fn tsc() -> u64 {
    // SAFETY: RDTSC has no side effects and every x86_64 CPU has it.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// The TSC's rate in Hz over one window of about [`WINDOW_NS`] of HPET time.
fn measure() -> u64 {
    let (start_ns, start_tsc) = (hpet_nanos(), tsc());
    let mut now = start_ns;
    while now - start_ns < WINDOW_NS {
        // Spin on the TSC between HPET reads: under VirtualBox a vCPU's TSC
        // stalls while an HPET read is emulated, so back-to-back reads would
        // measure the stalls, not the TSC.
        let from = tsc();
        while tsc().wrapping_sub(from) < SPIN_TICKS {
            core::hint::spin_loop();
        }
        now = hpet_nanos();
    }
    let ticks = tsc().saturating_sub(start_tsc);
    (u128::from(ticks) * 1_000_000_000 / u128::from(now - start_ns)) as u64
}

/// Whether a measured TSC rate is one to trust: within 1% of the rate the
/// hypervisor reports (CPUID 0x40000010), when it reports one, and within 0.5% of
/// the window before.
fn settled(rate: u64, last: u64, reported_hz: Option<u64>) -> bool {
    let steady = rate != 0 && rate.abs_diff(last) * 200 < last;
    steady && reported_hz.is_none_or(|hz| rate.abs_diff(hz) * 100 < hz)
}

/// Switches `nanos` to the TSC if the command line says `clock=tsc` and the TSC
/// keeps the right rate against the HPET, and logs the outcome. VirtualBox runs a
/// guest's TSC slow, and steadily so, for a while after power-on, so a rate counts
/// only once two windows in a row agree and, when the hypervisor reports the TSC's
/// rate, match it; with none in [`WINDOWS`] the HPET stays the clock. Runs on the
/// boot CPU before the others start.
pub fn select_clock(cmdline: &str) {
    if cmdline
        .split_whitespace()
        .any(|word| word == "clock=icount")
    {
        set_zero(0);
        TSC_MULT.store(1 << 32, Ordering::Release);
        ICOUNT.store(true, Ordering::Release);
        log!("clock: the TSC at 1 GHz, as QEMU's icount runs it");
        return;
    }
    if !cmdline.split_whitespace().any(|word| word == "clock=tsc") {
        return;
    }
    let reported_hz = cpufeat::hypervisor_tsc_khz().map(|khz| u64::from(khz) * 1000);
    let mut last = measure();
    for window in 2..=WINDOWS {
        let rate = measure();
        if settled(rate, last, reported_hz) {
            let mult = (((1_000_000_000u128) << 32) / u128::from(rate)) as u64;
            set_zero(0);
            // Last, so a reader that sees the rate also sees the zero points.
            TSC_MULT.store(mult.max(1), Ordering::Release);
            log!(
                "clock: the TSC, {} MHz, settled after {window} windows (hypervisor says {} MHz)",
                rate / 1_000_000,
                reported_hz.map_or(0, |hz| hz / 1_000_000)
            );
            return;
        }
        last = rate;
    }
    log!(
        "clock: the TSC never settled (last {} MHz); the HPET stays the clock",
        last / 1_000_000
    );
}

fn set_zero(cpu: usize) {
    let nanos = hpet_nanos();
    TSC_ZERO[cpu].store(tsc(), Ordering::Relaxed);
    NANOS_ZERO[cpu].store(nanos, Ordering::Relaxed);
}

/// Gives CPU `cpu` its TSC zero point, if the TSC is the clock. Each CPU runs it
/// once as it starts, before it reads the clock.
pub fn start_cpu(cpu: usize) {
    if TSC_MULT.load(Ordering::Acquire) != 0 && cpu < MAX_CPUS {
        set_zero(cpu);
    }
}

/// On an icount boot, moves the clock forward to the next whole second; anywhere
/// else, does nothing. For the icount boots' tick-aligned start: the clock's zero
/// is an HPET read, and the firmware and boot-drive reads before the kernel are
/// host-timed, so at the aligned tick the clock stood at a different fraction of a
/// millisecond in each boot. Whole milliseconds of it (the supervisor's timers)
/// then ran out on different ticks, and the observe boots flipped between two
/// summaries. From a whole second, every boot's clock reads the same fractions.
pub fn round_up_to_second() {
    if !ICOUNT.load(Ordering::Acquire) {
        return;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let now = nanos();
        let target = (now / NANOS_PER_SECOND + 1) * NANOS_PER_SECOND;
        for zero in &NANOS_ZERO {
            zero.fetch_add(target - now, Ordering::Relaxed);
        }
        LATEST.fetch_max(target, Ordering::Relaxed);
    });
}

/// Nanoseconds since the HPET was enabled.
pub fn nanos() -> u64 {
    let mult = TSC_MULT.load(Ordering::Acquire);
    if mult == 0 {
        return hpet_nanos();
    }
    // With interrupts off, the thread cannot move to another CPU between reading
    // this CPU's zero point and its TSC.
    let now = x86_64::instructions::interrupts::without_interrupts(|| {
        let cpu = percpu::index();
        let ticks = tsc().saturating_sub(TSC_ZERO[cpu].load(Ordering::Relaxed));
        let scaled = (u128::from(ticks) * u128::from(mult)) >> 32;
        NANOS_ZERO[cpu].load(Ordering::Relaxed) + scaled as u64
    });
    now.max(LATEST.fetch_max(now, Ordering::Relaxed))
}

fn hpet_nanos() -> u64 {
    let ticks = u128::from(main_counter());
    (ticks * u128::from(PERIOD_FS.load(Ordering::Relaxed)) / FEMTOS_PER_NANO) as u64
}

pub fn frequency_hz() -> u64 {
    1_000_000_000_000_000 / PERIOD_FS.load(Ordering::Relaxed).max(1)
}
