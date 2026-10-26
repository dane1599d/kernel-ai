//! Spike S12-1 `cc-wasm` (ADR 0012): xcc's self-hosted compiler `cc.wasm` on wasmi,
//! with `wasihost` for its WASI calls and `kailink` to link each object into a
//! session. Shared by the host tests (`tests/`) and the ring-3 program `ccspike`
//! of boot case `test=ccspike`. Never merged.

#![no_std]

extern crate alloc;
#[cfg(feature = "trace")]
extern crate std;

pub mod compiler;
pub mod corpus;
pub mod engine;
pub mod rhai_bench;
pub mod run;
pub mod session;

/// Nanoseconds from some fixed point.
pub type Clock = fn() -> u64;

pub use compiler::{Compiled, Compiler};
pub use session::{Ran, Session};
pub use wasmi::CompilationMode;
