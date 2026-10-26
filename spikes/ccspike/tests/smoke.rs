//! A first look: compile one entry with cc.wasm on the host and link it.

use spike_ccspike::{CompilationMode, Compiler, Session, corpus};
use std::sync::OnceLock;
use std::time::Instant;

fn now() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

fn cc_wasm() -> Option<Vec<u8>> {
    let path = std::env::var("CCWASM").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/ccwasm/cc.wasm").into()
    });
    std::fs::read(path).ok()
}

#[test]
fn compiles_links_and_runs_one_entry() {
    let Some(wasm) = cc_wasm() else {
        eprintln!("no cc.wasm: run `cargo xtask ccwasm` (Linux) first");
        return;
    };
    let headers: &[(&str, &[u8])] = &[("kai.h", corpus::KAI_H.as_bytes())];
    let (cc, ns) = Compiler::new(&wasm, headers, CompilationMode::Eager, now).unwrap();
    eprintln!("translate {} ms", ns / 1_000_000);
    let source = format!("{}{}", corpus::prelude(0), corpus::CORPUS[0].source);
    let out = cc.compile(&source, now).unwrap();
    eprintln!(
        "compile {} us, fuel {}, slices {}, memory {} KiB, object {} bytes",
        out.ns / 1000,
        out.fuel,
        out.slices,
        out.memory_bytes / 1024,
        out.object.len()
    );
    std::fs::write(std::env::temp_dir().join("entry1.o"), &out.object).unwrap();
    let mut session = Session::new(CompilationMode::Eager).unwrap();
    let ran = session.eval(&out.object, Some("__kai_run_1"), now).unwrap();
    assert_eq!(ran.result, Some(7));
}
