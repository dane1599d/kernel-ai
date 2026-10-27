//! Test service: spins forever without blocking. The kernel must kill it when it
//! exceeds its CPU burst limit, and the supervisor must restart it. With
//! `idle=1` it sleeps instead, so a case can hold many processes at once
//! without loading the CPUs.

#![no_std]
#![no_main]

use user::{arg, args, println, sys};

user::entry!(main);

fn main() -> i32 {
    let args = args();
    if arg(&args, "idle") == Some("1") {
        println!("up; idle");
        loop {
            sys::sleep_ms(60_000);
        }
    }
    println!(
        "up (restart {}); spinning",
        arg(&args, "restart").unwrap_or("0")
    );
    let mut counter = 0u64;
    loop {
        counter = core::hint::black_box(counter.wrapping_add(1));
    }
}
