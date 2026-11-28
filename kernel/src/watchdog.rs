//! The hardware watchdog (ADR 0008 §5): QEMU's i6300esb, so a trial slot that
//! hangs with interrupts off still resets, and R counts the lost boot.
//!
//! The software watchdogs (the scheduler's, a soft-lockup check) need a CPU
//! that still takes interrupts. This one counts down in the device: once
//! [`arm`]ed it resets the machine unless it is petted, and only CPU 0's tick
//! pets it, and only after every online CPU has ticked since the last pet. A
//! CPU stuck with interrupts off, any CPU, so stops the petting.
//!
//! The device's registers, as Intel's 6300ESB datasheet and QEMU's
//! `hw/watchdog/wdt_i6300esb.c` give them:
//! - configuration space 0x60 (16 bits): bit 5 clear resets on the second
//!   stage's timeout, bit 2 clear counts at about 1 kHz, bits 1:0 set raise no
//!   interrupt at the first stage's;
//! - configuration space 0x68 (8 bits): bit 1 counts, bit 0 locks that until a
//!   reset (never set here; [`bind`] reads it back, in case firmware did);
//! - BAR0 0x00 and 0x04 (32 bits): each stage's count, 20 bits;
//! - BAR0 0x0C (16 bits): 0x80 then 0x86 unlock the next write to any BAR0
//!   register; bit 8 there restarts the first stage, bit 9 clears the
//!   timed-out flag.
//!
//! QEMU takes the configuration registers only at their own width. A tick
//! that finds [`ESB`] held skips its pet rather than wait, so a pet never
//! lands inside [`arm`]'s unlock sequence.
//!
//! On a boot of slot A or B ([`crate::slots::this_slot`]), [`bind`] arms it
//! at once, so a trial that hangs in a driver's init, the supervisor's start
//! or user space resets too (#1571's review). The lever, once it has read the
//! record, keeps it armed for a trial and disarms it for a committed slot.
//!
//! The rule that every online CPU ticks needs a tick on every online CPU:
//! tickless idle (`timer.rs`) must have [`on_tick`] skip an idle CPU first.
//!
//! The board's TCO timer (P7) goes behind the same [`arm`], [`disarm`] and
//! [`on_tick`].

use crate::{log, memory, percpu, slots, smp, timer};
use bootslot::Slot;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts;

const CONFIG: u64 = 0x60;
const LOCK: u64 = 0x68;
/// Configuration: reset on timeout, about 1 kHz, no first-stage interrupt.
const CONFIG_RESET_1KHZ: u16 = 0x0003;
const LOCK_ENABLE: u8 = 1 << 1;
const LOCK_LOCKED: u8 = 1 << 0;

const TIMER1: u64 = 0x00;
const TIMER2: u64 = 0x04;
const RELOAD: u64 = 0x0c;
const UNLOCK1: u16 = 0x80;
const UNLOCK2: u16 = 0x86;
const RELOAD_RESTART: u16 = 1 << 8;
const RELOAD_TIMED_OUT: u16 = 1 << 9;
/// A stage's count is 20 bits.
const STAGE_MAX: u32 = (1 << 20) - 1;

/// How long a trial boot may go unpetted: long enough for the seconds-long
/// stalls KVM runners give a CPU (decisions.md, CI 2026-10-24), short beside
/// ADR 0008 §5's 300 s. `watchdog_s=` sets it for a test.
pub const TRIAL_S: u32 = 30;

/// The bound device: its configuration space and BAR0, mapped.
struct Esb {
    config: u64,
    bar: u64,
}

impl Esb {
    fn config16(&self, offset: u64, value: u16) {
        // SAFETY: `config` maps this function's 4 KiB of ECAM, uncached.
        unsafe { ((self.config + offset) as *mut u16).write_volatile(value) }
    }

    fn config8(&self, offset: u64, value: u8) {
        // SAFETY: as in config16.
        unsafe { ((self.config + offset) as *mut u8).write_volatile(value) }
    }

    fn read_config8(&self, offset: u64) -> u8 {
        // SAFETY: as in config16.
        unsafe { ((self.config + offset) as *const u8).read_volatile() }
    }

    fn bar16(&self, offset: u64, value: u16) {
        // SAFETY: `bar` maps the device's 16-byte BAR0, uncached.
        unsafe { ((self.bar + offset) as *mut u16).write_volatile(value) }
    }

    fn bar32(&self, offset: u64, value: u32) {
        // SAFETY: as in bar16.
        unsafe { ((self.bar + offset) as *mut u32).write_volatile(value) }
    }

    /// The next BAR0 write takes effect.
    fn unlock(&self) {
        self.bar16(RELOAD, UNLOCK1);
        self.bar16(RELOAD, UNLOCK2);
    }

    fn restart(&self) {
        self.unlock();
        self.bar16(RELOAD, RELOAD_RESTART);
    }

    /// Each stage counts half of `ms`, at 32,768 PCI clocks (about 0.98 ms)
    /// a count.
    fn set_ms(&self, ms: u32) {
        let stage = (ms / 2).clamp(1, STAGE_MAX);
        self.unlock();
        self.bar32(TIMER1, stage);
        self.unlock();
        self.bar32(TIMER2, stage);
    }
}

static ESB: Mutex<Option<Esb>> = Mutex::new(None);
static ARMED: AtomicBool = AtomicBool::new(false);
/// Firmware left it counting and locked: it cannot be stopped until a reset,
/// so it stays armed and petted, and [`disarm`] leaves it so.
static LOCKED_ON: AtomicBool = AtomicBool::new(false);
/// The CPUs that have ticked since the last pet, one bit each.
static TICKED: AtomicU32 = AtomicU32::new(0);
const _: () = assert!(percpu::MAX_CPUS <= 32, "TICKED has a bit per CPU");

/// Takes the i6300esb whose configuration space `config` maps, with BAR0 at
/// `bar` (`bar_len` bytes), from the PCI walk: stopped, set to reset on its
/// timeout, its timed-out flag cleared; then, on a boot of slot A or B, armed
/// for [`trial_s`].
///
/// 0x68 is read back after it is cleared: firmware that locked the timer
/// while it counted leaves it counting, and it is then petted from here on
/// rather than reported stopped (#1571's review); one locked while stopped
/// can never be armed, and is refused.
pub fn bind(config: u64, bar: u64, bar_len: u64) -> Result<&'static str, &'static str> {
    if bar_len < 16 {
        return Err("BAR0 is smaller than its registers");
    }
    let bar = memory::map_physical(bar, 16, crate::virtio::hal::DEVICE)?;
    let esb = Esb { config, bar };
    let lock = interrupts::without_interrupts(|| {
        let mut device = ESB.lock();
        esb.config16(CONFIG, CONFIG_RESET_1KHZ);
        esb.config8(LOCK, 0);
        let lock = esb.read_config8(LOCK);
        esb.unlock();
        esb.bar16(RELOAD, RELOAD_TIMED_OUT | RELOAD_RESTART);
        if lock & LOCK_ENABLE != 0 {
            TICKED.store(0, Ordering::Relaxed);
            LOCKED_ON.store(true, Ordering::Release);
            ARMED.store(true, Ordering::Release);
        }
        if lock & (LOCK_ENABLE | LOCK_LOCKED) != LOCK_LOCKED {
            *device = Some(esb);
        }
        lock
    });
    match (lock & LOCK_ENABLE != 0, lock & LOCK_LOCKED != 0) {
        (true, _) => return Ok("locked on by firmware, so petted from now"),
        (false, true) => return Err("locked off by firmware, so it can never be armed"),
        (false, false) => {}
    }
    if matches!(slots::this_slot(), Some(Slot::A | Slot::B)) {
        arm(trial_s());
        return Ok("armed until the lever reads the record");
    }
    Ok("stopped")
}

/// Whether the countdown runs.
pub fn armed() -> bool {
    ARMED.load(Ordering::Acquire)
}

/// The time [`arm`] gives a trial boot: `watchdog_s=` on the command line, or
/// [`TRIAL_S`].
pub fn trial_s() -> u32 {
    crate::cmdline()
        .split_whitespace()
        .find_map(|word| word.strip_prefix("watchdog_s=")?.parse().ok())
        .filter(|&s: &u32| (1..=STAGE_MAX / 500).contains(&s))
        .unwrap_or(TRIAL_S)
}

/// Starts the countdown: the machine resets `seconds` from the last pet.
/// Whether a watchdog was there to arm.
pub fn arm(seconds: u32) -> bool {
    let armed = interrupts::without_interrupts(|| {
        let device = ESB.lock();
        let Some(esb) = device.as_ref() else {
            return false;
        };
        esb.set_ms(seconds.saturating_mul(1000));
        esb.restart();
        TICKED.store(0, Ordering::Relaxed);
        esb.config8(LOCK, LOCK_ENABLE);
        ARMED.store(true, Ordering::Release);
        true
    });
    if armed {
        log!("watchdog: armed for {seconds} s");
    }
    armed
}

/// Stops the countdown, if it runs and can be stopped.
pub fn disarm() {
    if LOCKED_ON.load(Ordering::Acquire) {
        return;
    }
    let disarmed = interrupts::without_interrupts(|| {
        let device = ESB.lock();
        let Some(esb) = device.as_ref() else {
            return false;
        };
        let was = ARMED.swap(false, Ordering::AcqRel);
        esb.restart();
        esb.config8(LOCK, 0);
        was
    });
    if disarmed {
        log!("watchdog: disarmed");
    }
}

/// From every CPU's timer interrupt: note this CPU's tick, and on CPU 0, once
/// a second, pet the watchdog if every online CPU has ticked since the last
/// pet.
pub fn on_tick(cpu_ticks: u64) {
    if !ARMED.load(Ordering::Relaxed) {
        return;
    }
    let index = percpu::index();
    let bit = 1u32 << index;
    if TICKED.load(Ordering::Relaxed) & bit == 0 {
        TICKED.fetch_or(bit, Ordering::Relaxed);
    }
    if index != 0 || !cpu_ticks.is_multiple_of(timer::rate_hz()) {
        return;
    }
    let online = smp::online().clamp(1, percpu::MAX_CPUS);
    let all = u32::MAX >> (32 - online as u32);
    if TICKED.load(Ordering::Relaxed) & all != all {
        return;
    }
    // Held only by arm, disarm and bind, which mask interrupts: skip this pet.
    let Some(device) = ESB.try_lock() else {
        return;
    };
    if let Some(esb) = device.as_ref()
        && ARMED.load(Ordering::Relaxed)
    {
        esb.restart();
        TICKED.store(0, Ordering::Relaxed);
    }
}
