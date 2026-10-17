//! Test service: allocates memory without limit. The kernel must kill it when it
//! exceeds its page quota, and the supervisor must restart it.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use user::{arg, args, println, sys};

user::entry!(main);

const CHUNK: usize = 64 * 1024;

fn main() -> i32 {
    let args = args();
    println!(
        "up (restart {}); allocating",
        arg(&args, "restart").unwrap_or("0")
    );
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    loop {
        // Touch every page so the memory is really used.
        chunks.push(alloc::vec![0xA5; CHUNK]);
        if chunks.len().is_multiple_of(16) {
            println!("holding {} KiB", chunks.len() * CHUNK / 1024);
        }
        sys::sleep_ms(1);
    }
}
