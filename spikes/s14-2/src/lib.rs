//! S14-2: Wasmtime's runtime in a ring-3 process. The engine's configuration
//! is here, so the host tool that makes the `.cwasm` files and the process
//! that loads them cannot differ. The platform glue Wasmtime asks of a
//! `no_std` embedder is in the program (`user/src/bin/wasmtimespike`), which
//! has the system calls.

#![no_std]
extern crate alloc;

use alloc::string::String;
use wasmtime::{Config, Engine, Instance, Module, Store};

/// The two ways a module is compiled ahead of time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Native x86-64 code, baseline CPU (no host features inferred): runs
    /// from memory the process made read-execute.
    Native,
    /// Pulley, Wasmtime's interpreter: no executable memory.
    Pulley,
}

/// The configuration both sides use. No virtual-memory tricks: memories are
/// plain committed pages, and traps are checks, not signals.
pub fn config(kind: Kind) -> wasmtime::Result<Config> {
    let mut config = Config::new();
    config.target(match kind {
        Kind::Native => "x86_64-kai-none",
        Kind::Pulley => "pulley64",
    })?;
    config.signals_based_traps(false);
    // No copy-on-write images: this build has no virtual-memory platform.
    config.memory_init_cow(false);
    config.memory_reservation(0);
    config.memory_guard_size(0);
    config.memory_reservation_for_growth(0);
    config.guard_before_linear_memory(false);
    Ok(config)
}

/// What one load and run measured, in nanoseconds of `clock`.
#[derive(Debug, Default, Clone, Copy)]
pub struct Report {
    pub cwasm_bytes: usize,
    pub verify_ns: u64,
    pub deserialize_ns: u64,
    pub instantiate_ns: u64,
    pub call_ns: u64,
    pub fib: i32,
    pub sum: i32,
    pub mix: i32,
}

/// Checks `signature` over `cwasm` against `public_key`, then loads it and
/// calls `fib(20)`, `sum(8)` and `mix(rounds)`.
pub fn run(
    kind: Kind,
    cwasm: &[u8],
    signature: &[u8],
    public_key: &[u8],
    rounds: i32,
    clock: &dyn Fn() -> u64,
) -> Result<Report, String> {
    let text = |e: wasmtime::Error| alloc::format!("{e:#}");
    let mut report = Report {
        cwasm_bytes: cwasm.len(),
        ..Report::default()
    };
    let t = clock();
    let key = ed25519_compact::PublicKey::from_slice(public_key).map_err(|_| "bad public key")?;
    let sig = ed25519_compact::Signature::from_slice(signature).map_err(|_| "bad signature")?;
    key.verify(cwasm, &sig)
        .map_err(|_| "the signature does not verify")?;
    report.verify_ns = clock() - t;

    let t = clock();
    let engine = Engine::new(&config(kind).map_err(text)?).map_err(text)?;
    // SAFETY: `cwasm` is what this crate's host tool made, and its signature
    // verified just above; deserialize trusts what it is given.
    let module = unsafe { Module::deserialize(&engine, cwasm) }.map_err(text)?;
    report.deserialize_ns = clock() - t;

    let t = clock();
    let mut store = Store::new(&engine, ());
    let instance = Instance::new(&mut store, &module, &[]).map_err(text)?;
    report.instantiate_ns = clock() - t;

    let fib = instance
        .get_typed_func::<i32, i32>(&mut store, "fib")
        .map_err(text)?;
    let sum = instance
        .get_typed_func::<i32, i32>(&mut store, "sum")
        .map_err(text)?;
    let mix = instance
        .get_typed_func::<i32, i32>(&mut store, "mix")
        .map_err(text)?;
    report.fib = fib.call(&mut store, 20).map_err(text)?;
    report.sum = sum.call(&mut store, 8).map_err(text)?;
    let t = clock();
    report.mix = mix.call(&mut store, rounds).map_err(text)?;
    report.call_ns = clock() - t;
    Ok(report)
}

/// What `mix(rounds)` must answer, computed here.
pub fn mix_expected(rounds: u32) -> i32 {
    let mut x: u32 = 12345;
    let mut i: u32 = 0;
    loop {
        x = (x ^ (x >> 15))
            .wrapping_mul(2_246_822_519)
            .wrapping_add(3_266_489_917);
        i += 1;
        if i >= rounds {
            return x as i32;
        }
    }
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
    (local.get $s))
  (func (export "mix") (param $n i32) (result i32)
    (local $x i32) (local $i i32)
    (local.set $x (i32.const 12345))
    (loop $again
      (local.set $x
        (i32.add
          (i32.mul
            (i32.xor (local.get $x) (i32.shr_u (local.get $x) (i32.const 15)))
            (i32.const 2246822519))
          (i32.const 3266489917)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br_if $again (i32.lt_u (local.get $i) (local.get $n))))
    (local.get $x)))
"#;
