//! Runs component versions on wasmi as [`livecore::Instance`]s. Nothing else in
//! the system names a wasmi type.
//!
//! Each version gets its own engine, since a wasmi engine keeps compiled code until
//! it is dropped: dropping a version frees all of it. Modules are compiled eagerly
//! (lazy translation would charge the first call for compiling), start functions
//! and post-2.0 proposals are refused, and every call gets fresh fuel, so a loop
//! that never ends is a fault rather than a hang.
//!
//! Two hazards in dev builds, where ring-3 programs are built under `cargo xtask
//! test`: wasmi's default dispatch grows the native stack with every instruction
//! when debug assertions are on (hence `portable-dispatch`), and its host-function
//! glue crashes when compiled without optimisation (hence opt-level 2 for this
//! crate). This crate's API is therefore not generic and not inline, so no wasmi
//! generic is compiled in an unoptimised caller.

#![no_std]

extern crate alloc;

mod instance;
#[cfg(test)]
mod tests;

use alloc::string::ToString;
pub use instance::WasmInstance;
use livecore::{Fault, Reason};
use wasmi::{CompilationMode, Config, EnforcedLimits, Engine, Module};

/// Resources one instance may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Fuel for one `kai_handle` call.
    pub fuel_call: u64,
    /// Fuel for any other call, and for instantiation.
    pub fuel_life: u64,
    /// Most linear memory, in bytes.
    pub memory_bytes: usize,
}

/// Where a version's log lines go; `None` drops them.
pub type LogSink = fn(&str);

/// Longest log line kept, in bytes.
pub const MAX_LOG_BYTES: usize = 160;
/// Most log lines one call may write.
pub const MAX_LOG_LINES: u32 = 8;

/// The engine settings every version is compiled with.
fn config() -> Config {
    let mut config = Config::default();
    config
        .consume_fuel(true)
        .compilation_mode(CompilationMode::Eager)
        .allow_start_fn(false)
        .ignore_custom_sections(true)
        .set_max_recursion_depth(1024)
        .set_max_cached_stacks(1)
        .enforced_limits(EnforcedLimits::strict())
        // What wasm32-unknown-unknown emits, and nothing newer.
        .wasm_mutable_global(true)
        .wasm_sign_extension(true)
        .wasm_saturating_float_to_int(true)
        .wasm_multi_value(true)
        .wasm_bulk_memory(true)
        .wasm_reference_types(true)
        .wasm_multi_memory(false)
        .wasm_tail_call(false)
        .wasm_extended_const(false)
        .wasm_custom_page_sizes(false)
        .wasm_wide_arithmetic(false);
    config
}

/// Compiles and instantiates one version. `log` receives its log lines.
#[inline(never)]
pub fn load(wasm: &[u8], limits: Limits, log: Option<LogSink>) -> Result<WasmInstance, Fault> {
    if wasm.len() > kabi::live::MAX_ARTIFACT {
        return Err(Fault::new(Reason::Compile, "the module is too large"));
    }
    let engine = Engine::new(&config());
    let module = Module::new(&engine, wasm)
        .map_err(|error| Fault::new(Reason::Compile, error.to_string()))?;
    WasmInstance::new(&engine, &module, limits, log)
}
