//! Spike: Rhai evaluating in ring 3.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::ToString;
use user::println;

user::entry!(main);

fn main() -> i32 {
    let engine = rhai::Engine::new();
    match engine.eval::<i64>("fn add(a, b) { a + b } let x = 40; add(x, 2)") {
        Ok(value) => println!("rhai says {value}"),
        Err(error) => println!("rhai failed: {}", error.to_string()),
    }
    0
}
