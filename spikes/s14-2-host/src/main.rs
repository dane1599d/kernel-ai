//! Compiles the spike's module for native code and for Pulley, signs each
//! `.cwasm`, and writes them with the public key beside the spike crate.

use ed25519_compact::{KeyPair, Seed};
use spike_wasmtime_nostd::Kind;

fn main() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../s14-2");
    let wasm = wat::parse_str(spike_wasmtime_nostd::WAT).expect("wat");
    let keys = KeyPair::from_seed(Seed::new([7; 32]));
    std::fs::write(format!("{dir}/public.key"), keys.pk.as_ref()).expect("key");
    for (name, kind) in [("native", Kind::Native), ("pulley", Kind::Pulley)] {
        let config = spike_wasmtime_nostd::config(kind).expect("config");
        let engine = wasmtime::Engine::new(&config).expect("engine");
        let bytes = engine.precompile_module(&wasm).expect("compile");
        let sig = keys.sk.sign(&bytes, None);
        std::fs::write(format!("{dir}/{name}.cwasm"), &bytes).expect("write");
        std::fs::write(format!("{dir}/{name}.cwasm.sig"), sig.as_ref()).expect("sig");
        println!("{name}: {} bytes", bytes.len());
    }
}
