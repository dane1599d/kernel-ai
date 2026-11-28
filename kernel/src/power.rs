//! Rebooting without AML (ADR 0007 section 8): the FADT's reset register, else
//! the 8042's reset line when the machine has one, else a triple fault. Each way
//! gets 100 ms to take effect before the next is tried. `reboot=8042` on the
//! command line skips the FADT register and `reboot=fault` both, for the tests.

use crate::{cmdline, hpet, log, memory, platform, port};
use acpitables::fadt::{Reset, Space};
use core::sync::atomic::{AtomicU64, Ordering};

/// How long one way gets to reset the machine before the next is tried.
const SETTLE_NS: u64 = 100_000_000;
/// How long the other CPUs get to stop before it is said safe to switch off.
const HALT_WAIT_NS: u64 = 100_000_000;

/// The memory-space reset register mapped at boot: its physical address (0:
/// none) and where. A reboot runs with interrupts off and may not map.
static RESET_PHYS: AtomicU64 = AtomicU64::new(0);
static RESET_VIRT: AtomicU64 = AtomicU64::new(0);

/// Maps the FADT's reset register if it is in memory space; once, at boot.
pub fn init(platform: &acpitables::Platform) {
    let Some(reset) = platform.reset.filter(|r| r.register.space == Space::Memory) else {
        return;
    };
    if let Err(message) = map_reset(reset) {
        log!("power: cannot map the reset register ({message}); a reboot skips it");
    }
}

pub(crate) fn map_reset(reset: Reset) -> Result<(), &'static str> {
    let virt = memory::map_physical(reset.register.address, 1, crate::virtio::hal::DEVICE)?;
    RESET_VIRT.store(virt, Ordering::Relaxed);
    RESET_PHYS.store(reset.register.address, Ordering::Release);
    Ok(())
}

/// Where the machine cannot be turned off: shows that it is safe to switch
/// off, on the console's screen again if `desktop` held it, and halts this
/// CPU with interrupts off (ADR 0007 section 8). Never returns.
pub fn safe_to_switch_off() -> ! {
    // Every CPU is about to stop with interrupts off, which an armed trial's
    // watchdog would turn into a reset (#1571's review). Before the others
    // halt, so none of them holds the watchdog's lock.
    crate::watchdog::disarm();
    x86_64::instructions::interrupts::disable();
    // Every other CPU stops too, so neither `fs` nor an app runs on while the
    // screen says it is safe. Nothing is half written: POWER_OFF was refused
    // as Busy while `fs` held a call, and `fs` commits before it answers.
    crate::smp::halt_others();
    let (halted, others) = crate::smp::others_halted(HALT_WAIT_NS);
    let had_desktop = crate::console::suspended();
    crate::console::resume();
    if halted == others {
        log!("power: every other CPU stopped ({others})");
    } else {
        log!("power: only {halted} of {others} other CPUs stopped");
    }
    if had_desktop && !crate::console::suspended() {
        log!("power: the console has the screen back from the desktop");
    }
    log!("power: it is now safe to switch off the computer");
    loop {
        x86_64::instructions::hlt();
    }
}

/// Reboots the machine. Never returns. Allocates nothing: the fault path ends
/// here (`fault::reset`), perhaps from a fault inside the allocator, or with
/// its lock held by a CPU that is halted now.
pub fn reboot() -> ! {
    x86_64::instructions::interrupts::disable();
    let skip = cmdline()
        .split_whitespace()
        .find_map(|word| word.strip_prefix("reboot="))
        .unwrap_or("");
    let platform = platform::get();
    let reset = platform.and_then(|p| p.reset).filter(|_| skip.is_empty());
    if let Some(reset) = reset {
        log!(
            "power: rebooting through the FADT reset register ({})",
            ResetWrite(reset)
        );
        write_reset(reset);
        settle();
        log!("power: the reset register did not reset the machine");
    }
    let has_8042 = platform.is_some_and(|p| !p.no_8042);
    if has_8042 && skip != "fault" {
        log!("power: rebooting through the 8042 reset line");
        pulse_8042();
        settle();
        log!("power: the 8042 did not reset the machine");
    }
    log!("power: rebooting by a triple fault");
    triple_fault()
}

/// The reset register's write, for `reboot`'s line: formatted as it is
/// printed, never on the heap.
struct ResetWrite(Reset);

impl core::fmt::Display for ResetWrite {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let reset = self.0;
        let space = match reset.register.space {
            Space::Io => "I/O port",
            Space::Memory => "memory at",
            Space::PciConfig => "PCI configuration",
            Space::Other(_) => "an unsupported space at",
        };
        write!(
            f,
            "write {:#x} to {space} {:#x}",
            reset.value, reset.register.address
        )
    }
}

/// Writes the reset value. Takes no lock: memory space only where `init` mapped.
pub(crate) fn write_reset(reset: Reset) {
    let address = reset.register.address;
    match reset.register.space {
        // SAFETY: the firmware names this port as the machine's reset register.
        Space::Io => unsafe { port::outb(address as u16, reset.value) },
        Space::Memory => {
            let mapped = RESET_PHYS.load(Ordering::Acquire) == address;
            let virt = RESET_VIRT.load(Ordering::Relaxed);
            // RESET_PHYS is 0 when nothing was mapped: a register at 0 is not it.
            if mapped && virt != 0 {
                // SAFETY: the firmware names this byte as the reset register,
                // mapped uncached at boot by `map_reset`.
                unsafe { (virt as *mut u8).write_volatile(reset.value) };
            }
        }
        Space::PciConfig => {
            let device = ((address >> 32) & 0x1F) as u32;
            let function = ((address >> 16) & 0x7) as u32;
            let offset = (address & 0xFF) as u32;
            let select = 0x8000_0000 | device << 11 | function << 8 | (offset & 0xFC);
            // SAFETY: the legacy configuration mechanism, selecting the
            // register the firmware names, on segment 0 bus 0.
            unsafe {
                port::outl(0xCF8, select);
                port::outb(0xCFC + (offset & 3) as u16, reset.value);
            }
        }
        Space::Other(_) => {}
    }
}

/// Pulses the CPU reset line through the 8042's output port (command 0xFE).
fn pulse_8042() {
    const STATUS_COMMAND: u16 = 0x64;
    const INPUT_FULL: u8 = 1 << 1;
    let start = hpet::nanos();
    // SAFETY: reading the 8042 status port has no side effect.
    while unsafe { port::inb(STATUS_COMMAND) } & INPUT_FULL != 0 {
        if hpet::nanos() - start > SETTLE_NS {
            break;
        }
    }
    // SAFETY: the reset pulse is what is wanted here.
    unsafe { port::outb(STATUS_COMMAND, 0xFE) };
}

fn settle() {
    let start = hpet::nanos();
    while hpet::nanos() - start < SETTLE_NS {
        core::hint::spin_loop();
    }
}

/// An empty IDT, then an exception: the CPU cannot deliver it, nor the double
/// fault that follows, and shuts down, which resets the machine.
fn triple_fault() -> ! {
    let empty = x86_64::structures::DescriptorTablePointer {
        limit: 0,
        base: x86_64::VirtAddr::new(0),
    };
    // SAFETY: deliberately breaks interrupt delivery to force a reset.
    unsafe {
        x86_64::instructions::tables::lidt(&empty);
        core::arch::asm!("int3", options(noreturn));
    }
}
