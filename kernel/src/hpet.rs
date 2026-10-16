//! HPET main counter: the kernel's reference clock.

use crate::memory;
use core::sync::atomic::{AtomicU64, Ordering};
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

fn read(offset: u64) -> u64 {
    // SAFETY: BASE maps the HPET register block uncached.
    unsafe { ((BASE.load(Ordering::Relaxed) + offset) as *const u64).read_volatile() }
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

/// Nanoseconds since the HPET was enabled.
pub fn nanos() -> u64 {
    let ticks = u128::from(read(MAIN_COUNTER));
    (ticks * u128::from(PERIOD_FS.load(Ordering::Relaxed)) / FEMTOS_PER_NANO) as u64
}

pub fn frequency_hz() -> u64 {
    1_000_000_000_000_000 / PERIOD_FS.load(Ordering::Relaxed).max(1)
}
