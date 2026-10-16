//! Frame-pointer backtraces. The kernel is built with `force-frame-pointers=yes`.

use crate::lines::Describe;
use crate::log;

const MAX_FRAMES: usize = 24;
/// Kernel frames live in the higher half; anything below is not a kernel stack.
const HIGHER_HALF: u64 = 0xFFFF_8000_0000_0000;

/// The caller's frame pointer. Always inlined so it reads the caller's own RBP.
#[inline(always)]
pub fn current_rbp() -> u64 {
    let rbp: u64;
    // SAFETY: reads a register only.
    unsafe {
        core::arch::asm!("mov {}, rbp", out(reg) rbp, options(nomem, nostack, preserves_flags))
    };
    rbp
}

/// Logs return addresses by walking the saved-RBP chain starting at `rbp`.
pub fn print_from(mut rbp: u64) {
    for depth in 0..MAX_FRAMES {
        if rbp < HIGHER_HALF || !rbp.is_multiple_of(8) {
            return;
        }
        // SAFETY: rbp is an aligned higher-half address taken from a frame-pointer chain.
        let (saved_rbp, return_addr) = unsafe {
            let frame = rbp as *const u64;
            (frame.read(), frame.add(1).read())
        };
        if return_addr == 0 {
            return;
        }
        // A return address points after the call; step back into the call instruction.
        log!("  #{depth:<2} {}", Describe(return_addr - 1));
        rbp = saved_rbp;
    }
    log!("  ... (stopped after {MAX_FRAMES} frames)");
}
