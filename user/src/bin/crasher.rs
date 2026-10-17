//! Test service: works briefly, then reads an unmapped address. The kernel must
//! kill it with a page fault, and the supervisor must restart it.

#![no_std]
#![no_main]

use user::{arg, args, println, sys};

user::entry!(main);

/// Unmapped and in user space, so the fault is a plain not-present page fault.
const UNMAPPED: u64 = 0x0000_0DEA_D000_0000;

fn main() -> i32 {
    let args = args();
    let restart = arg(&args, "restart").unwrap_or("0");
    let delay = arg(&args, "after_ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    println!("up (restart {restart}); crashing in {delay} ms");
    sys::sleep_ms(delay);
    crash()
}

#[inline(never)]
fn crash() -> i32 {
    // SAFETY: deliberately faults; the kernel kills the process here.
    unsafe { core::ptr::read_volatile(UNMAPPED as *const u64) as i32 }
}
