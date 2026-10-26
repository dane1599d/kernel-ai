//! The whole spike on the host (release builds only; times are the host's, the
//! boot case's are the ones that count):
//! cargo test --release -p spike-ccspike --test measure -- --nocapture

use spike_ccspike::run::{self, Env};
use std::sync::OnceLock;
use std::time::Instant;

fn now() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

#[test]
fn spike_s12_1() {
    let path = std::env::var("CCWASM").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/ccwasm/cc.wasm").into()
    });
    let Ok(wasm) = std::fs::read(&path) else {
        eprintln!("no cc.wasm at {path}: run `cargo xtask ccwasm` (Linux) first");
        return;
    };
    let env = Env {
        cc_wasm: &wasm,
        clock: now,
        heap: None,
        repeats: 5,
        rhai: !cfg!(debug_assertions),
    };
    let verdict = run::all(&env, &mut |line| eprintln!("{line}"));
    assert!(verdict.failures.is_empty(), "{:#?}", verdict.failures);
}
