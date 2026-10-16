//! 1 kHz tick from the local APIC timer in one-shot mode.
//!
//! Each tick re-arms the timer for an absolute deadline measured on the HPET, so
//! interrupt latency never accumulates into drift. The same one-shot mechanism
//! will later allow tickless idle.

use crate::{apic, hpet, log, percpu};
use core::sync::atomic::{AtomicU64, Ordering};

pub const HZ: u64 = 1000;
const PERIOD_NS: u64 = 1_000_000_000 / HZ;
const CALIBRATION_NS: u64 = 10_000_000;

/// CPU 0's tick count: the system-wide clock the tests measure.
static TICKS: AtomicU64 = AtomicU64::new(0);
/// Local APIC timer counts per millisecond at divide-by-16. Every local APIC timer
/// runs from the same clock, so the boot CPU's calibration serves all CPUs.
static APIC_PER_MS: AtomicU64 = AtomicU64::new(0);

/// Starts an application processor's 1 kHz tick using the boot CPU's calibration.
pub fn init_ap() {
    apic::timer_configure();
    let now = hpet::nanos();
    percpu::this().tick_start_ns.store(now, Ordering::Relaxed);
    arm(now + PERIOD_NS);
}

/// Calibrates the local APIC timer against the HPET and starts ticking.
/// Interrupts must still be disabled.
pub fn init() -> Result<(), &'static str> {
    apic::timer_configure();
    apic::timer_start(u32::MAX);
    let start = hpet::nanos();
    while hpet::nanos() - start < CALIBRATION_NS {
        core::hint::spin_loop();
    }
    let counted = u64::from(u32::MAX - apic::timer_remaining());
    apic::timer_start(0);
    let per_ms = counted * 1_000_000 / CALIBRATION_NS;
    if per_ms == 0 {
        return Err("timer: local APIC timer did not count during calibration");
    }
    APIC_PER_MS.store(per_ms, Ordering::Relaxed);
    let now = hpet::nanos();
    percpu::this().tick_start_ns.store(now, Ordering::Relaxed);
    arm(now + PERIOD_NS);
    log!(
        "timer: APIC {} counts/ms, HPET {} Hz, ticking at {HZ} Hz",
        per_ms,
        hpet::frequency_hz()
    );
    Ok(())
}

/// Called from the timer interrupt.
pub fn on_tick() {
    let cpu = percpu::this();
    let ticks = cpu.ticks.fetch_add(1, Ordering::Relaxed) + 1;
    if percpu::index() == 0 {
        TICKS.fetch_add(1, Ordering::Relaxed);
    }
    arm(cpu.tick_start_ns.load(Ordering::Relaxed) + (ticks + 1) * PERIOD_NS);
}

fn arm(deadline_ns: u64) {
    let remaining = deadline_ns.saturating_sub(hpet::nanos());
    let count = remaining * APIC_PER_MS.load(Ordering::Relaxed) / 1_000_000;
    apic::timer_start(count.clamp(1, u64::from(u32::MAX)) as u32);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Sleeps (halting between interrupts) until `ticks` more ticks have passed or
/// `timeout_ns` of HPET time has elapsed. Returns the HPET time it took.
pub fn wait_ticks(ticks: u64, timeout_ns: u64) -> Result<u64, &'static str> {
    let (target, start) = (self::ticks() + ticks, hpet::nanos());
    while self::ticks() < target {
        if hpet::nanos() - start > timeout_ns {
            return Err("timer: ticks stopped arriving");
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
    Ok(hpet::nanos() - start)
}

/// Boot check: 100 ticks take 100 ms of HPET time, within 20%.
pub fn boot_check() -> Result<(), &'static str> {
    let elapsed = wait_ticks(100, 2_000_000_000)?;
    if !(80_000_000..=120_000_000).contains(&elapsed) {
        return Err("timer: 100 ticks did not take about 100 ms");
    }
    log!("self-test: 100 ticks in {} us of HPET time", elapsed / 1000);
    Ok(())
}

/// M3 exit test: 60 000 ticks must take 60 s of HPET time, within 2%.
pub fn sixty_second_check() -> Result<(), &'static str> {
    const TICKS_60S: u64 = 60 * HZ;
    log!("timer: measure-start, counting {TICKS_60S} ticks");
    let elapsed = wait_ticks(TICKS_60S, 90_000_000_000)?;
    log!(
        "timer: measure-end, {} ms of HPET time",
        elapsed / 1_000_000
    );
    let error_ppm = elapsed.abs_diff(60_000_000_000) / 60_000;
    log!("timer: error {} ppm (limit 20000)", error_ppm);
    if error_ppm > 20_000 {
        return Err("timer: 1 kHz tick drifted more than 2% over 60 s");
    }
    Ok(())
}
