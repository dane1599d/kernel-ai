//! The engine settings both Stores use: wasmhost's proposal set (wasmhost/src/lib.rs),
//! with fuel, and the compilation mode under test. `strict` applies wasmhost's
//! `EnforcedLimits::strict()`; cc.wasm has more active data segments than it allows
//! (1,000), so the compiler's Store, which runs only the SHA-pinned cc.wasm, does
//! without it.

use wasmi::{CompilationMode, Config, EnforcedLimits};

pub fn config(mode: CompilationMode, strict: bool) -> Config {
    let mut config = Config::default();
    config
        .consume_fuel(true)
        .compilation_mode(mode)
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
