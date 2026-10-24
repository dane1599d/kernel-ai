//! Rebooting without AML (ADR 0007 section 8): the FADT's reset register, else
//! the 8042's reset line when the machine has one, else a triple fault. Each way
//! gets 100 ms to take effect before the next is tried. `reboot=8042` on the
//! command line skips the FADT register and `reboot=fault` both, for the tests.

use crate::{cmdline, hpet, log, memory, platform, port};
use acpitables::fadt::{Reset, Space};

/// How long one way gets to reset the machine before the next is tried.
const SETTLE_NS: u64 = 100_000_000;

/// Reboots the machine. Never returns.
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
            describe(reset)
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

fn describe(reset: Reset) -> alloc::string::String {
    let space = match reset.register.space {
        Space::Io => "I/O port",
        Space::Memory => "memory at",
        Space::PciConfig => "PCI configuration",
        Space::Other(_) => "an unsupported space at",
    };
    alloc::format!(
        "write {:#x} to {space} {:#x}",
        reset.value,
        reset.register.address
    )
}

fn write_reset(reset: Reset) {
    let address = reset.register.address;
    match reset.register.space {
        // SAFETY: the firmware names this port as the machine's reset register.
        Space::Io => unsafe { port::outb(address as u16, reset.value) },
        Space::Memory => {
            if let Ok(virt) = memory::map_physical(address, 1, crate::virtio::hal::DEVICE) {
                // SAFETY: the firmware names this byte as the reset register,
                // mapped uncached just now.
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
