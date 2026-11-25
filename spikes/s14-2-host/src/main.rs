//! Compiles the spike's module for Pulley and writes `hello.cwasm`.

fn main() {
    let config = spike_wasmtime_nostd::config().expect("config");
    let engine = wasmtime::Engine::new(&config).expect("engine");
    let wasm = wat::parse_str(spike_wasmtime_nostd::WAT).expect("wat");
    let bytes = engine.precompile_module(&wasm).expect("compile");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../s14-2/hello.cwasm");
    std::fs::write(path, &bytes).expect("write");
    println!("{} bytes -> {path}", bytes.len());
}
