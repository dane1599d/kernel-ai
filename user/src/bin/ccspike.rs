//! Spike S12-1 (ADR 0012), boot case `test=ccspike`: runs `spike_ccspike::run::all`
//! in ring 3 with cc.wasm embedded, printing its `ccspike:` lines, then the peak
//! heap against `cc`'s planned 96 MiB. Arguments: `repeats=N` compiles per entry
//! (5), `rhai=0` skips the Rhai loops. Exits 0 when every check the run makes
//! passed. Built only on the never-merged branch spike/cc-wasm, after
//! `cargo xtask ccwasm`.

#![no_std]
#![no_main]

use spike_ccspike::run::{self, Env};
use user::{arg, args, println, sys};

user::entry!(main);

static CC_WASM: &[u8] = include_bytes!("../../../target/ccwasm/cc.wasm");
/// `cc`'s plan line: `pages=24576`.
const CEILING_BYTES: u64 = 96 << 20;

fn heap() -> usize {
    user::heap_bytes() as usize
}

fn main() -> i32 {
    let args = args();
    let env = Env {
        cc_wasm: CC_WASM,
        clock: sys::time_ns,
        heap: Some(heap),
        repeats: arg(&args, "repeats")
            .and_then(|n| n.parse().ok())
            .unwrap_or(5),
        rhai: arg(&args, "rhai") != Some("0"),
    };
    let start = sys::time_ns();
    let verdict = run::all(&env, &mut |line| println!("{line}"));
    let brk = sys::brk(0)
        .unwrap_or(0)
        .saturating_sub(kabi::layout::HEAP_BASE);
    println!(
        "ccspike: peak heap (brk) {} KiB of the 96 MiB ceiling ({}%), live {} KiB; whole run {} ms",
        brk / 1024,
        brk * 100 / CEILING_BYTES,
        user::heap_bytes() / 1024,
        (sys::time_ns() - start) / 1_000_000
    );
    if verdict.failures.is_empty() {
        println!("ccspike: all checks passed");
        0
    } else {
        println!("ccspike: {} checks failed", verdict.failures.len());
        1
    }
}
