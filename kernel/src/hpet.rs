//! HPET main counter: the kernel's reference clock.
//!
//! A machine with no HPET (some boards ship it off or hidden; P6 G57) uses the
//! FADT's ACPI PM timer in its place: 3.579545 MHz, 24 or 32 bits, read through
//! an I/O port and carried into 64 bits like a 32-bit HPET. Everything below
//! that says the HPET means that counter then, and the log names it.
//!
//! With the TSC as the clock, `nanos` reads it instead, scaled by a rate measured
//! against the HPET at boot. Every timer tick reads the clock a few times, and
//! every HPET read is a trip to the hypervisor: under VirtualBox on Hyper-V, with
//! 8 vCPUs ticking at 1 kHz, those trips take all the CPU time there is, and
//! under KVM they held a download to 18 MB/s where the TSC gives 29 (spike P5-S2).
//! Which clock (the boot log says which, and why):
//! - `clock=tsc`: the TSC wherever it calibrates (`cargo xtask vbox` boots so);
//! - `clock=hpet`: the HPET always;
//! - neither: the TSC under KVM ([`cpufeat::default_clock`]), whether or not it
//!   is invariant or the hypervisor reports its rate (P6 G67); the HPET
//!   everywhere else, TCG included.
//!
//! The TSC is kept only if it calibrates, and then agrees with the HPET within
//! [`DRIFT_PPM`] between two windows of [`DRIFT_WINDOW_NS`]; otherwise the HPET
//! stays the clock. A TSC that changed rate after those windows would go
//! unseen; under KVM it runs at a fixed rate the host sets, and `clock=hpet`
//! keeps the HPET.
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
use acpitables::fadt::PM_TIMER_HZ;
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
/// The latest time any CPU returned. CPUs whose TSC started apart keep zero
/// points of their own ([`start_cpu`]), each from an HPET read that takes
/// microseconds, so their clocks can disagree by that much, and no caller may
/// see time go back.
static LATEST: AtomicU64 = AtomicU64::new(0);
/// One window of measuring the TSC against the HPET, and at most how many.
const WINDOW_NS: u64 = 25_000_000;
const WINDOWS: u32 = 200;
/// TSC ticks spun between two HPET reads while calibrating.
const SPIN_TICKS: u64 = 1 << 20;
/// After calibrating, the TSC must keep its rate against the HPET within this
/// over this long, or the HPET stays the clock. 100 ms keeps an HPET read's few
/// microseconds of jitter to about 10 ppm of it.
pub const DRIFT_PPM: u64 = 100;
pub const DRIFT_WINDOW_NS: u64 = 100_000_000;

/// The last main-counter value read without tearing, by any CPU.
static LAST_TICKS: AtomicU64 = AtomicU64::new(0);
/// Main-counter reads that were not near the last good value, so the halves were
/// read instead: torn reads, which QEMU before 10.0 gives at each 43 s wrap of
/// the low half.
static SPLIT_READS: AtomicU64 = AtomicU64::new(0);
/// Whether the main counter is 64 bits wide (CAPABILITIES bit 13); a 32-bit one
/// is carried into 64 bits ([`acpitables::hpet_counter::extend32`]).
static WIDE: AtomicBool = AtomicBool::new(true);
/// The PM timer's port when it stands in for a missing HPET; 0 with an HPET.
static PM_PORT: AtomicU64 = AtomicU64::new(0);
/// The PM timer counts 32 bits (TMR_VAL_EXT), not 24.
static PM_BITS32: AtomicBool = AtomicBool::new(false);
/// The PM timer's period: 10^15 / 3,579,545 Hz, within 0.1 ppb.
const PM_PERIOD_FS: u64 = 279_365_115;
const _: () = assert!(PM_PERIOD_FS.abs_diff(1_000_000_000_000_000 / PM_TIMER_HZ) <= 1);

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
/// Reads of the HPET's main counter since boot: each one exits to the
/// hypervisor in a VM, so M6 counts them.
static READS: AtomicU64 = AtomicU64::new(0);

/// How many times the HPET's main counter has been read.
pub fn reads() -> u64 {
    READS.load(Ordering::Relaxed)
}

/// Whether `nanos` reads the TSC (`clock=tsc` settled) rather than the HPET.
pub fn tsc_clock() -> bool {
    TSC_MULT.load(Ordering::Acquire) != 0
}

/// The PM timer's count as read, 24 or 32 bits.
fn pm_raw(port: u16) -> u32 {
    let mut register = x86_64::instructions::port::PortReadOnly::<u32>::new(port);
    // SAFETY: the FADT names this port as the PM timer's, a read-only counter.
    let raw = unsafe { register.read() };
    match PM_BITS32.load(Ordering::Relaxed) {
        true => raw,
        false => raw & 0xFF_FFFF,
    }
}

/// The PM timer's count, read and then carried into 64 bits from the last good
/// value. The port is read first: a reader preempted between loading
/// LAST_TICKS and the read would carry a count more than half a wrap ahead as
/// one behind, 4.69 s in the past (#1660's review). Read in this order, the
/// last value can only be a little ahead, which the carry allows.
fn pm_counter(port: u16) -> u64 {
    let raw = pm_raw(port);
    let last = LAST_TICKS.load(Ordering::Relaxed);
    match PM_BITS32.load(Ordering::Relaxed) {
        true => acpitables::hpet_counter::extend32(raw, last),
        false => acpitables::hpet_counter::extend24(raw, last),
    }
}

fn main_counter() -> u64 {
    READS.fetch_add(1, Ordering::Relaxed);
    let pm = PM_PORT.load(Ordering::Relaxed) as u16;
    if pm != 0 {
        let ticks = pm_counter(pm);
        LAST_TICKS.fetch_max(ticks, Ordering::Relaxed);
        return ticks;
    }
    let last = LAST_TICKS.load(Ordering::Relaxed);
    let (ticks, split) = if WIDE.load(Ordering::Relaxed) {
        acpitables::hpet_counter::read(
            || read(MAIN_COUNTER),
            || read32(MAIN_COUNTER),
            || read32(MAIN_COUNTER + 4),
            last,
        )
    } else {
        // A 32-bit read cannot tear: only the carry is needed.
        let carried = acpitables::hpet_counter::extend32(read32(MAIN_COUNTER), last);
        (carried, false)
    };
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

/// From CPU 0's tick: with a 32-bit counter (or the PM timer's 24 bits, which
/// wrap every 4.7 s) and the TSC as the clock, reads the counter once a second
/// so the carry never misses a wrap. The HPET clock reads it every tick anyway;
/// a 64-bit counter needs nothing.
pub fn on_tick(tick: u64) {
    if !WIDE.load(Ordering::Relaxed) && tsc_clock() && tick.is_multiple_of(crate::timer::rate_hz())
    {
        main_counter();
    }
}

fn write(offset: u64, value: u64) {
    // SAFETY: as in read.
    unsafe { ((BASE.load(Ordering::Relaxed) + offset) as *mut u64).write_volatile(value) }
}

/// The reference counter: the HPET when the firmware has one, else the FADT's
/// PM timer (P6 G57).
pub fn init(platform: &acpitables::Platform) -> Result<(), &'static str> {
    if let Some(phys) = platform.hpet.filter(|&phys| phys != 0) {
        return init_hpet(phys);
    }
    let Some(pm) = platform.pm_timer else {
        return Err("no HPET and no ACPI PM timer; a PIT fallback is not implemented yet");
    };
    PM_BITS32.store(pm.bits32, Ordering::Relaxed);
    PERIOD_FS.store(PM_PERIOD_FS, Ordering::Relaxed);
    WIDE.store(false, Ordering::Relaxed);
    // The carry starts from where the counter is now, as for a 32-bit HPET.
    LAST_TICKS.store(u64::from(pm_raw(pm.port)), Ordering::Relaxed);
    PM_PORT.store(u64::from(pm.port), Ordering::Release);
    log!(
        "hpet: none; the ACPI PM timer at port {:#x} ({} bits, {PM_TIMER_HZ} Hz) is the reference clock",
        pm.port,
        if pm.bits32 { 32 } else { 24 }
    );
    Ok(())
}

/// What the reference counter is, for the log: "HPET" or "PM timer".
pub fn reference() -> &'static str {
    match PM_PORT.load(Ordering::Relaxed) {
        0 => "HPET",
        _ => "PM timer",
    }
}

fn init_hpet(phys: u64) -> Result<(), &'static str> {
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::WRITE_THROUGH
        | PageTableFlags::NO_EXECUTE;
    BASE.store(memory::map_physical(phys, 0x400, flags)?, Ordering::Relaxed);
    let capabilities = read(CAPABILITIES);
    let period = capabilities >> 32;
    if period == 0 || period > MAX_PERIOD_FS {
        return Err("HPET: invalid counter period");
    }
    PERIOD_FS.store(period, Ordering::Relaxed);
    write(CONFIGURATION, read(CONFIGURATION) | ENABLE);
    if !acpitables::hpet_counter::counter_is_64bit(capabilities) {
        WIDE.store(false, Ordering::Relaxed);
        // The carry starts from where the counter is now, not from 0.
        LAST_TICKS.store(u64::from(read32(MAIN_COUNTER)), Ordering::Relaxed);
        log!("hpet: a 32-bit main counter; carried into 64 bits in software");
    }
    Ok(())
}

fn tsc() -> u64 {
    // SAFETY: RDTSC has no side effects and every x86_64 CPU has it.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// The TSC's rate in Hz over one window of about [`WINDOW_NS`] of HPET time.
fn measure() -> u64 {
    measure_over(WINDOW_NS)
}

/// The TSC's rate in Hz over about `window_ns` of HPET time.
fn measure_over(window_ns: u64) -> u64 {
    let (start_ns, start_tsc) = (hpet_nanos(), tsc());
    let mut now = start_ns;
    while now - start_ns < window_ns {
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

/// Picks the clock (see the module's docs) and logs which, and why. Runs on the
/// boot CPU before the others start.
pub fn select_clock(cmdline: &str) {
    let r = reference();
    let said = |word: &str| cmdline.split_whitespace().any(|w| w == word);
    if said("clock=icount") {
        set_zero(0);
        TSC_MULT.store(1 << 32, Ordering::Release);
        log!("clock: the TSC at 1 GHz, as QEMU's icount runs it (clock=icount)");
        return;
    }
    if said("clock=hpet") {
        log!("clock: the {r} (clock=hpet)");
        return;
    }
    let why = match said("clock=tsc") {
        true => "clock=tsc",
        false => match cpufeat::default_clock(
            cpufeat::hypervisor_signature(),
            cpufeat::invariant_tsc(),
            cpufeat::hypervisor_tsc_khz(),
        ) {
            cpufeat::DefaultClock::Tsc(why) => why,
            cpufeat::DefaultClock::Hpet(why) => {
                log!("clock: the {r} ({why})");
                return;
            }
        },
    };
    try_tsc(why);
}

/// Switches `nanos` to the TSC if it keeps the right rate against the HPET, and
/// logs the outcome with `why` it was tried. VirtualBox runs a guest's TSC slow,
/// and steadily so, for a while after power-on, so a rate counts only once two
/// windows in a row agree and, when the hypervisor reports the TSC's rate, match
/// it; with none in [`WINDOWS`] the HPET stays the clock. A settled rate must
/// then hold within [`DRIFT_PPM`] between two windows of [`DRIFT_WINDOW_NS`].
fn try_tsc(why: &str) {
    let r = reference();
    let reported_hz = cpufeat::hypervisor_tsc_khz().map(|khz| u64::from(khz) * 1000);
    let mut last = measure();
    for window in 2..=WINDOWS {
        let rate = measure();
        if settled(rate, last, reported_hz) {
            // Two windows of DRIFT_WINDOW_NS each, not the 25 ms rate that
            // settled against one: an HPET read's jitter is about 10 ppm of
            // 100 ms but four times that of 25 ms, which put 19 of 345 KVM
            // boots over DRIFT_PPM on a steady TSC (#1628's review).
            let base = measure_over(DRIFT_WINDOW_NS);
            let check = measure_over(DRIFT_WINDOW_NS);
            let ppm = cpufeat::drift_ppm(base, check);
            if ppm > DRIFT_PPM {
                log!(
                    "clock: the {r}: the TSC settled at {} MHz but then drifted {ppm} ppm against the {r} over {} ms, over {DRIFT_PPM} ({why})",
                    rate / 1_000_000,
                    DRIFT_WINDOW_NS / 1_000_000
                );
                return;
            }
            let mult = (((1_000_000_000u128) << 32) / u128::from(check)) as u64;
            set_zero(0);
            // Last, so a reader that sees the rate also sees the zero points.
            TSC_MULT.store(mult.max(1), Ordering::Release);
            log!(
                "clock: the TSC, {} MHz, settled after {window} windows and within {ppm} ppm of the {r} over {} ms ({why}; hypervisor says {} MHz)",
                check / 1_000_000,
                DRIFT_WINDOW_NS / 1_000_000,
                reported_hz.map_or(0, |hz| hz / 1_000_000)
            );
            return;
        }
        last = rate;
    }
    log!(
        "clock: the {r}: the TSC never settled (last {} MHz) ({why})",
        last / 1_000_000
    );
}

/// HPET reads per zero pair: the quickest is kept ([`zero_pair`]).
const ZERO_SAMPLES: usize = 8;

/// A TSC value and the HPET's nanoseconds at about that moment: of
/// [`ZERO_SAMPLES`] HPET reads, each between two TSC reads, the quickest
/// ([`acpitables::hpet_counter::best_pair`]), so one slow read neither puts a
/// zero point microseconds off nor makes a CPU keep a zero point of its own.
fn zero_pair() -> (u64, u64) {
    let samples = (0..ZERO_SAMPLES).map(|_| {
        let before = tsc();
        let nanos = hpet_nanos();
        (before, nanos, tsc())
    });
    acpitables::hpet_counter::best_pair(samples).unwrap_or_else(|| (tsc(), hpet_nanos()))
}

fn set_zero(cpu: usize) {
    let (ticks, nanos) = zero_pair();
    TSC_ZERO[cpu].store(ticks, Ordering::Relaxed);
    NANOS_ZERO[cpu].store(nanos, Ordering::Relaxed);
}

/// Gives CPU `cpu` its TSC zero point, if the TSC is the clock. Each CPU runs it
/// once as it starts, before it reads the clock.
///
/// The boot CPU's zero point when this CPU's TSC agrees with it
/// ([`acpitables::hpet_counter::shares_zero`]), as KVM's do: one offset per VM.
/// A zero point of its own, from a fresh HPET read, put each CPU's clock a few
/// microseconds from the others', and [`LATEST`] then held a CPU behind at the
/// leader's time: `time_ns` returned the same value across a 2-4 us run, and a
/// work process calibrated 200,000,000 iterations per ms (the tune flakes after
/// #1628). A TSC that started apart, as VirtualBox's do, still gets its own.
pub fn start_cpu(cpu: usize) {
    let mult = TSC_MULT.load(Ordering::Acquire);
    if mult == 0 || cpu >= MAX_CPUS || cpu == 0 {
        return;
    }
    let (now, hpet) = zero_pair();
    let ticks = now.wrapping_sub(TSC_ZERO[0].load(Ordering::Relaxed));
    // A TSC behind the boot CPU's zero wraps to a huge count and fails the check.
    let from_boot = NANOS_ZERO[0]
        .load(Ordering::Relaxed)
        .saturating_add(((u128::from(ticks) * u128::from(mult)) >> 32) as u64);
    if acpitables::hpet_counter::shares_zero(from_boot, hpet) {
        TSC_ZERO[cpu].store(TSC_ZERO[0].load(Ordering::Relaxed), Ordering::Relaxed);
        NANOS_ZERO[cpu].store(NANOS_ZERO[0].load(Ordering::Relaxed), Ordering::Relaxed);
    } else {
        TSC_ZERO[cpu].store(now, Ordering::Relaxed);
        NANOS_ZERO[cpu].store(hpet, Ordering::Relaxed);
        log!(
            "clock: cpu {cpu}'s TSC is {} ns from the boot CPU's against the HPET; it keeps a zero point of its own",
            from_boot.wrapping_sub(hpet) as i64
        );
    }
}

/// How far [`skip_for_test`] moved the clock on: added to every reading.
static SKIPPED_NS: AtomicU64 = AtomicU64::new(0);

/// Nanoseconds since the HPET was enabled.
pub fn nanos() -> u64 {
    clock_nanos() + SKIPPED_NS.load(Ordering::Relaxed)
}

/// Moves the clock on by `ns` on every CPU at once, as a host that stops the VM
/// for that long does: the `runaway-stall` case. Each CPU's ticks then come at
/// once until they catch up with it.
pub fn skip_for_test(ns: u64) {
    SKIPPED_NS.fetch_add(ns, Ordering::Relaxed);
}

fn clock_nanos() -> u64 {
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

/// Nanoseconds on the HPET's own counter, whatever the clock is: what the host
/// says has passed, which a TSC jump does not move (serial stamps, `stamp=ms`).
pub fn counter_nanos() -> u64 {
    hpet_nanos()
}

fn hpet_nanos() -> u64 {
    let ticks = u128::from(main_counter());
    (ticks * u128::from(PERIOD_FS.load(Ordering::Relaxed)) / FEMTOS_PER_NANO) as u64
}

pub fn frequency_hz() -> u64 {
    1_000_000_000_000_000 / PERIOD_FS.load(Ordering::Relaxed).max(1)
}
