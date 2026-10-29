//! The engine settings both Stores use: wasmhost's proposal set
//! (wasmhost/src/lib.rs), with fuel. `strict` applies wasmi's
//! `EnforcedLimits::strict()`. cc.wasm has 2,538 active data segments, over the
//! strict limit of 1,000 (S12-1), so the compiler's Store, which runs only the
//! SHA-pinned cc.wasm, goes without; session code keeps them (P4 plan, section
//! 4, decision 6).
//!
//! Translation is eager. Lazily, wasmi charges a function's translation to the
//! fuel of the call that first reaches it, and when the slice cannot pay, that
//! call fails outright instead of pausing for more fuel.

use wasmi::{CompilationMode, Config, EnforcedLimits};

pub fn config(strict: bool) -> Config {
    let mut config = Config::default();
    config
        .consume_fuel(true)
        .compilation_mode(CompilationMode::Eager)
        .allow_start_fn(false)
        .ignore_custom_sections(true)
        .set_max_recursion_depth(1024)
        .set_max_cached_stacks(1)
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
    if strict {
        config.enforced_limits(EnforcedLimits::strict());
    }
    config
}
