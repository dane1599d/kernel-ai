//! Local APIC and I/O APIC.
//!
//! The local APIC runs in one of two modes, the same on every CPU:
//! - xAPIC, its registers in a memory-mapped page: the default;
//! - x2APIC (P6 G48), its registers as MSRs 0x800 + offset / 16, with 32-bit
//!   IDs and a 64-bit ICR: used when firmware hands over in it (it cannot be
//!   turned off again without a reset), or when `apic=x2apic` asks and CPUID
//!   offers it. Each application processor switches to the boot CPU's mode.
//!
//! The I/O APIC's destination field stays 8 bits (no interrupt remapping), so
//! its interrupts go to an APIC ID of at most 254.

use crate::memory;
use acpitables::Platform;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
/// x2APIC MSRs start here: register `offset` is MSR `X2APIC_MSRS + offset / 16`.
const X2APIC_MSRS: u32 = 0x800;

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
/// Every CPU's local APIC is in x2APIC mode.
static X2APIC: AtomicBool = AtomicBool::new(false);

fn x2apic() -> bool {
    X2APIC.load(Ordering::Relaxed)
}

fn x2apic_msr(offset: u64) -> Msr {
    Msr::new(X2APIC_MSRS + (offset >> 4) as u32)
}

fn lapic_read(offset: u64) -> u32 {
    if x2apic() {
        // SAFETY: in x2APIC mode every register below is an MSR at its offset / 16.
        return unsafe { x2apic_msr(offset).read() } as u32;
    }
    let base = LAPIC.load(Ordering::Relaxed);
    // SAFETY: base maps the local APIC register page uncached; offset is a register.
    unsafe { ((base + offset) as *const u32).read_volatile() }
}

fn lapic_write(offset: u64, value: u32) {
    if x2apic() {
        // SAFETY: as in lapic_read.
        return unsafe { x2apic_msr(offset).write(u64::from(value)) };
    }
    let base = LAPIC.load(Ordering::Relaxed);
    // SAFETY: as in lapic_read.
    unsafe { ((base + offset) as *mut u32).write_volatile(value) }
}

/// This CPU's local APIC ID: 32 bits in x2APIC mode, the top byte of the
/// register in xAPIC mode.
fn local_id() -> u32 {
    match x2apic() {
        true => lapic_read(ID),
        false => lapic_read(ID) >> 24,
    }
}

/// Enables the boot CPU's local APIC, in the mode [`cpufeat::apic::mode`]
/// picks, and returns its APIC ID.
pub fn init_local(platform: &Platform) -> Result<u32, &'static str> {
    let mut msr = Msr::new(IA32_APIC_BASE);
    // SAFETY: IA32_APIC_BASE exists on every x86_64 CPU.
    let value = unsafe { msr.read() };
    let asked = crate::cmdline()
        .split_whitespace()
        .any(|w| w == "apic=x2apic");
    let (x2, why) =
        match cpufeat::apic::mode(value & X2APIC_ENABLE != 0, cpufeat::apic::offered(), asked) {
            cpufeat::apic::Mode::X2Apic(why) => (true, why),
            cpufeat::apic::Mode::XApic(why) => (false, why),
        };
    if x2 {
        // SAFETY: from xAPIC (or x2APIC) mode, setting both bits enters (or
        // keeps) x2APIC mode with the same base.
        unsafe { msr.write(value | APIC_GLOBAL_ENABLE | X2APIC_ENABLE) };
        X2APIC.store(true, Ordering::Relaxed);
    } else {
        // SAFETY: setting the global enable bit keeps the current base address.
        unsafe { msr.write(value | APIC_GLOBAL_ENABLE) };
        let base = memory::map_physical(platform.local_apic, 0x1000, DEVICE)?;
        LAPIC.store(base, Ordering::Relaxed);
    }
    lapic_write(TASK_PRIORITY, 0);
    lapic_write(SPURIOUS, 0x100 | u32::from(SPURIOUS_VECTOR));
    crate::log!("apic: {why}");
    Ok(local_id())
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
    apic_id: u32,
) -> Result<(), &'static str> {
    let apic_id = u8::try_from(apic_id)
        .ok()
        .filter(|&id| id != 0xff)
        .ok_or("APIC: the I/O APIC cannot reach an APIC ID above 254")?;
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

/// Enables an application processor's local APIC in the boot CPU's mode. In
/// xAPIC mode the register page is already mapped: every CPU sees its own
/// local APIC at the same address.
pub fn init_ap() -> u32 {
    let mut msr = Msr::new(IA32_APIC_BASE);
    let mode = match x2apic() {
        true => APIC_GLOBAL_ENABLE | X2APIC_ENABLE,
        false => APIC_GLOBAL_ENABLE,
    };
    // SAFETY: IA32_APIC_BASE exists on every x86_64 CPU; this sets the enable
    // bit, and in x2APIC mode the x2APIC bit too (the boot CPU found CPUID
    // offers it, and every CPU of the machine has the same local APIC).
    unsafe {
        let value = msr.read();
        msr.write(value | mode);
    }
    lapic_write(TASK_PRIORITY, 0);
    lapic_write(SPURIOUS, 0x100 | u32::from(SPURIOUS_VECTOR));
    local_id()
}

/// Sends a fixed interrupt with `vector` to the CPU whose local APIC has `lapic_id`.
pub fn send_ipi(lapic_id: u32, vector: u8) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        if x2apic() {
            // One 64-bit write, the destination in its top half; x2APIC has no
            // delivery status to wait on. A WRMSR to the x2APIC MSRs is not
            // serializing (SDM vol. 3, 11.12.3), so mfence; lfence first: a
            // shootdown's stores must be seen before its IPI lands.
            let icr = (u64::from(lapic_id) << 32) | u64::from(u32::from(vector) | ICR_ASSERT);
            // SAFETY: fences only; then the ICR's MSR, in x2APIC mode.
            unsafe {
                core::arch::asm!("mfence", "lfence", options(nostack, preserves_flags));
                x2apic_msr(ICR_LOW).write(icr);
            }
            return;
        }
        while lapic_read(ICR_LOW) & ICR_PENDING != 0 {
            core::hint::spin_loop();
        }
        lapic_write(ICR_HIGH, lapic_id << 24);
        lapic_write(ICR_LOW, u32::from(vector) | ICR_ASSERT);
    });
}
