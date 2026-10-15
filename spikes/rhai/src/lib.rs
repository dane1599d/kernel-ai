//! M0 spike: evaluate a Rhai script with no standard library.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};

const SCRIPT: &str = "fn add(a, b) { a + b } let x = 40; add(x, 2)";

/// Evaluates a small script that defines and calls a function. Expected result: 42.
pub fn run() -> Result<i64, String> {
    let engine = rhai::Engine::new();
    engine
        .eval::<i64>(SCRIPT)
        .map_err(|error| error.to_string())
}
