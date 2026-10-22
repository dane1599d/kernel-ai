//! The tick from the local APIC timer in one-shot mode: 1 kHz, or the rate `hz=`
//! on the command line gives (100 to 1000).
//!
//! Each tick re-arms the timer for an absolute deadline measured on the HPET, so
//! interrupt latency never accumulates into drift. The same one-shot mechanism
//! will later allow tickless idle. Under VirtualBox on Hyper-V every tick costs two
//! exits to VirtualBox's local APIC emulation, and 8 vCPUs ticking at 1 kHz leave
//! the guest a few percent of its CPU time; `cargo xtask vbox` boots with `hz=250`.

use crate::{apic, hpet, log, percpu};
use core::sync::atomic::{AtomicU64, Ordering};

/// The default tick rate, and the range `hz=` may pick from.
pub const HZ: u64 = 1000;
const MIN_HZ: u64 = 100;
static RATE_HZ: AtomicU64 = AtomicU64::new(HZ);

/// The tick rate in use.
pub fn rate_hz() -> u64 {
    RATE_HZ.load(Ordering::Relaxed)
}

fn period_ns() -> u64 {
    1_000_000_000 / rate_hz()
}
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
    arm(now + period_ns());
}

/// Calibrates the local APIC timer against the HPET and starts ticking.
/// Interrupts must still be disabled.
pub fn init() -> Result<(), &'static str> {
    let asked = crate::cmdline()
        .split_whitespace()
        .find_map(|word| word.strip_prefix("hz=")?.parse::<u64>().ok());
    if let Some(hz) = asked {
        RATE_HZ.store(hz.clamp(MIN_HZ, HZ), Ordering::Relaxed);
    }
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
    arm(now + period_ns());
    log!(
        "timer: APIC {} counts/ms, HPET {} Hz, ticking at {} Hz",
        per_ms,
        hpet::frequency_hz(),
        rate_hz()
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
    arm(cpu.tick_start_ns.load(Ordering::Relaxed) + (ticks + 1) * period_ns());
}

fn arm(deadline_ns: u64) {
    let remaining = deadline_ns.saturating_sub(hpet::nanos());
    let count = remaining * APIC_PER_MS.load(Ordering::Relaxed) / 1_000_000;
    apic::timer_start(count.clamp(1, u64::from(u32::MAX)) as u32);
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Halts until the next tick, so the caller goes on at the first instruction
/// after a timer interrupt. Under icount that puts what follows at the same place
/// against the ticks on every boot, however long the host-timed firmware and disk
/// reads before it took. Interrupts end as they began.
pub fn align_to_tick() {
    let enabled = x86_64::instructions::interrupts::are_enabled();
    let edge = ticks();
    while ticks() == edge {
        x86_64::instructions::interrupts::enable_and_hlt();
    }
    if !enabled {
        x86_64::instructions::interrupts::disable();
    }
}

/// Halts until the tick count reaches the next whole second, and returns that
/// tick. Like [`align_to_tick`], but every boot whose host-timed start-up ends
/// within the same second goes on at the same tick, so at the same absolute
/// virtual time too: aligning to the next tick alone let the M10 observe boots
/// start at tick 221 on one boot and 222 on another, and run apart from there.
pub fn align_to_second() -> u64 {
    let enabled = x86_64::instructions::interrupts::are_enabled();
    let edge = (ticks() / rate_hz() + 1) * rate_hz();
    while ticks() < edge {
        x86_64::instructions::interrupts::enable_and_hlt();
    }
    if !enabled {
        x86_64::instructions::interrupts::disable();
    }
    edge
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

/// Boot check: 100 ms of ticks take 100 ms of HPET time, within 20%.
pub fn boot_check() -> Result<(), &'static str> {
    let ticks = rate_hz() / 10;
    let elapsed = wait_ticks(ticks, 2_000_000_000)?;
    if !(80_000_000..=120_000_000).contains(&elapsed) {
        return Err("timer: 100 ms of ticks did not take about 100 ms");
    }
    log!(
        "self-test: {ticks} ticks in {} us of HPET time",
        elapsed / 1000
    );
    Ok(())
}

/// M3 exit test: 60 000 ticks must take 60 s of HPET time, within 2%.
pub fn sixty_second_check() -> Result<(), &'static str> {
    let ticks_60s = 60 * rate_hz();
    log!("timer: measure-start, counting {ticks_60s} ticks");
    let elapsed = wait_ticks(ticks_60s, 90_000_000_000)?;
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
