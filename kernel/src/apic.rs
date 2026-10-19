//! Local APIC (xAPIC, memory-mapped) and I/O APIC.

use crate::memory;
use acpitables::Platform;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::registers::model_specific::Msr;
use x86_64::structures::paging::PageTableFlags;

pub const TIMER_VECTOR: u8 = 0x20;
pub const KEYBOARD_VECTOR: u8 = 0x21;
pub const RESCHEDULE_VECTOR: u8 = 0x22;
pub const TLB_VECTOR: u8 = 0x23;
pub const HALT_VECTOR: u8 = 0x24;
pub const MOUSE_VECTOR: u8 = 0x25;
pub const SPURIOUS_VECTOR: u8 = 0xFF;

const IA32_APIC_BASE: u32 = 0x1B;
const APIC_GLOBAL_ENABLE: u64 = 1 << 11;
const X2APIC_ENABLE: u64 = 1 << 10;

// Local APIC register offsets.
const ID: u64 = 0x20;
const TASK_PRIORITY: u64 = 0x80;
const EOI: u64 = 0xB0;
const SPURIOUS: u64 = 0xF0;
const LVT_TIMER: u64 = 0x320;
const TIMER_INITIAL: u64 = 0x380;
const TIMER_CURRENT: u64 = 0x390;
const TIMER_DIVIDE: u64 = 0x3E0;
const DIVIDE_BY_16: u32 = 0b0011;
const ICR_LOW: u64 = 0x300;
const ICR_HIGH: u64 = 0x310;
const ICR_PENDING: u32 = 1 << 12;
const ICR_ASSERT: u32 = 1 << 14;

/// Uncached, writable, non-executable: the flags every register block needs.
const DEVICE: PageTableFlags = PageTableFlags::PRESENT
    .union(PageTableFlags::WRITABLE)
    .union(PageTableFlags::NO_CACHE)
    .union(PageTableFlags::WRITE_THROUGH)
    .union(PageTableFlags::NO_EXECUTE);

static LAPIC: AtomicU64 = AtomicU64::new(0);

fn lapic_read(offset: u64) -> u32 {
    let base = LAPIC.load(Ordering::Relaxed);
    // SAFETY: base maps the local APIC register page uncached; offset is a register.
    unsafe { ((base + offset) as *const u32).read_volatile() }
}

fn lapic_write(offset: u64, value: u32) {
    let base = LAPIC.load(Ordering::Relaxed);
    // SAFETY: as in lapic_read.
    unsafe { ((base + offset) as *mut u32).write_volatile(value) }
}

/// Enables this CPU's local APIC in xAPIC mode and returns its APIC ID.
pub fn init_local(platform: &Platform) -> Result<u8, &'static str> {
    let mut msr = Msr::new(IA32_APIC_BASE);
    // SAFETY: IA32_APIC_BASE exists on every x86_64 CPU.
    let value = unsafe { msr.read() };
    if value & X2APIC_ENABLE != 0 {
        return Err("APIC: firmware left x2APIC mode on; only xAPIC is supported so far");
    }
    // SAFETY: setting the global enable bit keeps the current base address.
    unsafe { msr.write(value | APIC_GLOBAL_ENABLE) };
    let base = memory::map_physical(platform.local_apic, 0x1000, DEVICE)?;
    LAPIC.store(base, Ordering::Relaxed);
    lapic_write(TASK_PRIORITY, 0);
    lapic_write(SPURIOUS, 0x100 | u32::from(SPURIOUS_VECTOR));
    Ok((lapic_read(ID) >> 24) as u8)
}

pub fn end_of_interrupt() {
    lapic_write(EOI, 0);
}

/// One-shot mode on `TIMER_VECTOR`, input clock divided by 16.
pub fn timer_configure() {
    lapic_write(TIMER_DIVIDE, DIVIDE_BY_16);
    lapic_write(LVT_TIMER, u32::from(TIMER_VECTOR));
}

/// Starts a one-shot countdown; 0 stops the timer.
pub fn timer_start(count: u32) {
    lapic_write(TIMER_INITIAL, count);
}

pub fn timer_remaining() -> u32 {
    lapic_read(TIMER_CURRENT)
}

/// Routes an ISA IRQ through the I/O APIC to `vector` on the CPU with `apic_id`.
pub fn route_isa_irq(
    platform: &Platform,
    irq: u8,
    vector: u8,
    apic_id: u8,
) -> Result<(), &'static str> {
    let route = platform.isa_route(irq);
    let io_apic = platform
        .io_apic_for(route.gsi)
        .ok_or("APIC: no I/O APIC handles this interrupt")?;
    let base = memory::map_physical(io_apic.address, 0x20, DEVICE)?;
    let register = 0x10 + 2 * (route.gsi - io_apic.gsi_base);
    let low = u32::from(vector)
        | (u32::from(route.active_low) << 13)
        | (u32::from(route.level_triggered) << 15);
    let write = |reg: u32, value: u32| {
        // SAFETY: IOREGSEL at base and IOWIN at base + 0x10 of a mapped I/O APIC.
        unsafe {
            (base as *mut u32).write_volatile(reg);
            ((base + 0x10) as *mut u32).write_volatile(value);
        }
    };
    write(register + 1, u32::from(apic_id) << 24);
    write(register, low);
    Ok(())
}

/// Enables an application processor's local APIC. The register page is already
/// mapped: every CPU sees its own local APIC at the same address.
pub fn init_ap() -> u8 {
    let mut msr = Msr::new(IA32_APIC_BASE);
    // SAFETY: IA32_APIC_BASE exists on every x86_64 CPU; this sets the enable bit.
    unsafe {
        let value = msr.read();
        msr.write(value | APIC_GLOBAL_ENABLE);
    }
    lapic_write(TASK_PRIORITY, 0);
    lapic_write(SPURIOUS, 0x100 | u32::from(SPURIOUS_VECTOR));
    (lapic_read(ID) >> 24) as u8
}

/// Sends a fixed interrupt with `vector` to the CPU whose local APIC has `lapic_id`.
pub fn send_ipi(lapic_id: u32, vector: u8) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        while lapic_read(ICR_LOW) & ICR_PENDING != 0 {
            core::hint::spin_loop();
        }
        lapic_write(ICR_HIGH, lapic_id << 24);
        lapic_write(ICR_LOW, u32::from(vector) | ICR_ASSERT);
    });
}
