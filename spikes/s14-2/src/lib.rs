//! S14-2: Wasmtime's runtime in a ring-3 process. The engine's configuration
//! is here, so the host tool that makes the `.cwasm` and the process that
//! loads it cannot differ; the platform glue Wasmtime asks of a `no_std`
//! embedder is in [`platform`].

#![no_std]
#![allow(unexpected_cfgs)]
extern crate alloc;

#[cfg(target_os = "kai")]
mod platform;

use alloc::string::String;
use alloc::vec::Vec;
use wasmtime::{Config, Engine, Instance, Module, Store};

/// The configuration both sides use. No virtual-memory tricks: the process
/// has `brk` and a few mappings, so memories are plain allocations, and traps
/// are checks, not signals.
pub fn config() -> wasmtime::Result<Config> {
    let mut config = Config::new();
    config.target("pulley64")?;
    config.signals_based_traps(false);
    config.memory_reservation(0);
    config.memory_guard_size(0);
    config.memory_reservation_for_growth(0);
    config.guard_before_linear_memory(false);
    Ok(config)
}

/// Loads `cwasm` and calls the export `fib` with `n`, and `sum` over the
/// first `len` bytes of memory.
pub fn run(cwasm: &[u8], n: i32, len: i32) -> Result<(i32, i32), String> {
    let text = |e: wasmtime::Error| alloc::format!("{e:#}");
    let engine = Engine::new(&config().map_err(text)?).map_err(text)?;
    // SAFETY: `cwasm` is the bytes this crate's host tool made, embedded in
    // the program; deserialize trusts what it is given.
    let module = unsafe { Module::deserialize(&engine, cwasm) }.map_err(text)?;
    let mut store = Store::new(&engine, ());
    let instance = Instance::new(&mut store, &module, &[]).map_err(text)?;
    let fib = instance
        .get_typed_func::<i32, i32>(&mut store, "fib")
        .map_err(text)?;
    let sum = instance
        .get_typed_func::<i32, i32>(&mut store, "sum")
        .map_err(text)?;
    let fib = fib.call(&mut store, n).map_err(text)?;
    let sum = sum.call(&mut store, len).map_err(text)?;
    Ok((fib, sum))
}

/// The module the host tool compiles, as text.
pub const WAT: &str = r#"
(module
  (memory (export "memory") 1 2)
  (data (i32.const 0) "\01\02\03\04\05\06\07\08")
  (func (export "fib") (param $n i32) (result i32)
    (local $a i32) (local $b i32) (local $t i32)
    (local.set $b (i32.const 1))
    (block $done
      (loop $next
        (br_if $done (i32.eqz (local.get $n)))
        (local.set $t (i32.add (local.get $a) (local.get $b)))
        (local.set $a (local.get $b))
        (local.set $b (local.get $t))
        (local.set $n (i32.sub (local.get $n) (i32.const 1)))
        (br $next)))
    (local.get $a))
  (func (export "sum") (param $len i32) (result i32)
    (local $i i32) (local $s i32)
    (loop $next
      (if (i32.lt_u (local.get $i) (local.get $len))
        (then
          (local.set $s (i32.add (local.get $s) (i32.load8_u (local.get $i))))
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $next))))
    (local.get $s)))
"#;

/// Not used by the process; keeps `Vec` in the imports for the glue.
#[allow(dead_code)]
type Bytes = Vec<u8>;
