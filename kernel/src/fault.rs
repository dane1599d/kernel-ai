//! A fault ends the boot by resetting the machine (ADR 0008, "fault"): a panic or
//! a failed check logs its reason and resets, so an unattended machine comes back
//! instead of halting. The reset is the FADT's reset register (ADR 0007 section 8,
//! `power::reboot`), with the 8042 and a triple fault behind it. `efi.rs` will
//! also write the reason to the `last_fault` variable, so the next boot can say
//! what happened.
//!
//! Test mode keeps isa-debug-exit, unless the command line says `panic=reset`
//! (the `panic-reset` case).

use crate::log;
use core::fmt::Display;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Faults seen while resetting from an earlier one.
static NESTED: AtomicUsize = AtomicUsize::new(0);

/// Whether a fault resets rather than ending a test run through isa-debug-exit.
pub fn resets(test_mode: bool) -> bool {
    !test_mode
        || crate::cmdline()
            .split_whitespace()
            .any(|word| word == "panic=reset")
}

/// Logs `reason` and resets the machine. Formats nothing on the heap, so a panic
/// in the allocator still gets here.
pub fn reset(reason: &dyn Display) -> ! {
    if NESTED.fetch_add(1, Ordering::SeqCst) > 0 {
        // A fault while resetting from another: reset without logging again.
        crate::power::reboot();
    }
    log!("fault: {reason}; resetting the machine");
    crate::efi_ab::on_fault();
    crate::power::reboot()
}
