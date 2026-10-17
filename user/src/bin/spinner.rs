//! Test service: spins forever without blocking. The kernel must kill it when it
//! exceeds its CPU burst limit, and the supervisor must restart it.

#![no_std]
#![no_main]

use user::{arg, args, println};

user::entry!(main);

fn main() -> i32 {
    let args = args();
    println!(
        "up (restart {}); spinning",
        arg(&args, "restart").unwrap_or("0")
    );
    let mut counter = 0u64;
    loop {
        counter = core::hint::black_box(counter.wrapping_add(1));
    }
}
