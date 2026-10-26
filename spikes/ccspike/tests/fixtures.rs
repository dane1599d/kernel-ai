//! Writes wcc objects for kailink's tests (kailink/tests/fixtures), compiled by
//! the pinned cc.wasm:
//! FIXTURES=kailink/tests/fixtures cargo test --release -p spike-ccspike --test fixtures -- --ignored

use spike_ccspike::{CompilationMode, Compiler, corpus};
use std::time::Instant;

fn now() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

#[test]
#[ignore]
fn write_fixtures() {
    let dir = std::env::var("FIXTURES").expect("FIXTURES=<dir>");
    let wasm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/ccwasm/cc.wasm"
    ))
    .unwrap();
    let headers: &[(&str, &[u8])] = &[("kai.h", corpus::KAI_H.as_bytes())];
    let (cc, _) = Compiler::new(&wasm, headers, CompilationMode::Lazy, now).unwrap();
    let mut sources: Vec<(String, String)> = corpus::CORPUS
        .iter()
        .filter(|e| {
            [
                "expr",
                "define-sq",
                "use-sq",
                "global",
                "extern",
                "print",
                "fnptr",
                "struct",
                "need",
                "notes",
                "uses-many",
                "fib",
            ]
            .contains(&e.name)
        })
        .map(|e| (e.name.replace('-', "_"), e.source.to_string()))
        .collect();
    for (i, (source, _)) in corpus::REDEFINE.iter().enumerate() {
        sources.push((format!("redefine{i}"), source.to_string()));
    }
    sources.push(("sum".into(), corpus::SUM.into()));
    for (name, source) in sources {
        let out = cc
            .compile(&format!("{}{source}", corpus::prelude(0)), now)
            .unwrap();
        std::fs::write(format!("{dir}/{name}.c"), &source).unwrap();
        std::fs::write(format!("{dir}/{name}.o"), &out.object).unwrap();
        eprintln!("{name}: {} bytes", out.object.len());
    }
    std::fs::write(format!("{dir}/kai.h"), corpus::KAI_H).unwrap();
}
