//! S14-2 (spike/wasmtime-nostd, never merged): Wasmtime's `no_std` runtime in a
//! ring-3 process. It checks the signature on `native.cwasm` (x86-64 code made
//! ahead of time on the host), loads it into memory made read-execute by
//! MEM_EXEC, and calls three exports; then the same with `pulley.cwasm`, the
//! interpreter's code. `fib(20)` must be 6765, `sum` over the 8-byte data
//! segment 36, and `mix` what `mix_expected` says. It prints the figures.

#![no_std]
#![no_main]

#[path = "wasmtimespike/platform.rs"]
mod platform;

use core::sync::atomic::Ordering;
use spike_wasmtime_nostd::{Kind, Report, mix_expected, run};
use user::{println, sys};

user::entry!(main);

static PUBLIC_KEY: &[u8] = include_bytes!("../../../spikes/s14-2/public.key");
static NATIVE: &[u8] = include_bytes!("../../../spikes/s14-2/native.cwasm");
static NATIVE_SIG: &[u8] = include_bytes!("../../../spikes/s14-2/native.cwasm.sig");
static PULLEY: &[u8] = include_bytes!("../../../spikes/s14-2/pulley.cwasm");
static PULLEY_SIG: &[u8] = include_bytes!("../../../spikes/s14-2/pulley.cwasm.sig");

/// Rounds of `mix`: about 20 ms native.
const ROUNDS: u32 = 20_000_000;

fn clock() -> u64 {
    sys::time_ns()
}

fn one(kind: Kind, cwasm: &[u8], sig: &[u8]) -> Result<Report, i32> {
    let name = match kind {
        Kind::Native => "native",
        Kind::Pulley => "pulley",
    };
    let heap_before = sys::brk(0).unwrap_or(0);
    match run(kind, cwasm, sig, PUBLIC_KEY, ROUNDS as i32, &clock) {
        Ok(r) if r.fib == 6765 && r.sum == 36 && r.mix == mix_expected(ROUNDS) => {
            println!(
                "wasmtime-nostd: {name}: cwasm {} B, verify {} us, deserialize {} us, instantiate {} us, mix({ROUNDS}) {} us; mapped now {} KiB, peak {} KiB",
                r.cwasm_bytes,
                r.verify_ns / 1000,
                r.deserialize_ns / 1000,
                r.instantiate_ns / 1000,
                r.call_ns / 1000,
                platform::LIVE.load(Ordering::Relaxed) / 1024,
                platform::PEAK.load(Ordering::Relaxed) / 1024,
            );
            println!(
                "wasmtime-nostd: {name}: heap grew {} KiB",
                sys::brk(0).unwrap_or(0).saturating_sub(heap_before) / 1024
            );
            Ok(r)
        }
        Ok(r) => {
            println!(
                "wasmtime-nostd: {name}: wrong answers: fib {} sum {} mix {}",
                r.fib, r.sum, r.mix
            );
            Err(1)
        }
        Err(why) => {
            println!("wasmtime-nostd: {name}: failed: {why}");
            Err(2)
        }
    }
}

fn main() -> i32 {
    // A signature that does not verify is refused before anything is loaded.
    let mut bad = [0u8; 64];
    bad.copy_from_slice(NATIVE_SIG);
    bad[0] ^= 1;
    if run(Kind::Native, NATIVE, &bad, PUBLIC_KEY, 1, &clock).is_ok() {
        println!("wasmtime-nostd: a bad signature was accepted");
        return 3;
    }
    println!("wasmtime-nostd: a bad signature is refused before the module loads");
    let native = match one(Kind::Native, NATIVE, NATIVE_SIG) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let pulley = match one(Kind::Pulley, PULLEY, PULLEY_SIG) {
        Ok(r) => r,
        Err(code) => return code,
    };
    println!(
        "wasmtime-nostd: native code ran mix {}x faster than Pulley",
        pulley.call_ns / native.call_ns.max(1)
    );
    println!("wasmtime-nostd: both answers right, native from read-execute pages");
    0
}
