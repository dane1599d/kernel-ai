//! Spike P5-S1's size baseline: `tlssize` without TLS, the same runtime and
//! allocation, so the difference in `.text` is what the TLS client adds. Never
//! booted. Not for merge.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use user::{println, sys};

user::entry!(main);

static INPUT: [u8; 64] = [0x16; 64];

fn main() -> i32 {
    let incoming: Vec<u8> = core::hint::black_box(&INPUT).to_vec();
    println!("tlsnone: {} {}", incoming.len(), sys::time_ns());
    0
}
