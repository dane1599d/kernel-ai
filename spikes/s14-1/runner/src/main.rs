//! `s14-run <mode> <wasm> [guest args...]`: runs the S14-1 engine as a WASI
//! guest and prints one `s14-host` line with what the host saw. The guest
//! prints its own `s14` lines (in-guest phase times).
//!
//! Modes:
//! - `compile-aot` / `compile-pulley`: CI's half, the component compiled
//!   ahead of time (`precompile_component`) to a `.cwasm` file, for Cranelift
//!   or for Pulley (`pulley64`). Its time is CI's cost, not the person's.
//! - `aot` / `pulley`: the person's half, the `.cwasm` loaded with
//!   `Component::deserialize_file`, as a CI-made, signed one would be, then
//!   run. A process of its own, so its peak RSS has no compiler in it.
//! - `wasmi`: the `wasm32-wasip1` core module on wasmi 2.0.3, as apps run
//!   today (wasmi has no component model).
//!
//! Peak guest memory is the largest linear memory any instance asked for,
//! seen through each runtime's resource limiter; peak RSS is the whole
//! runner's, from getrusage.

use std::time::Instant;

/// The largest linear memory the guest grew to, in bytes.
#[derive(Default)]
struct Peak(usize);

impl wasmtime::ResourceLimiter for Peak {
    fn memory_growing(
        &mut self,
        _: usize,
        desired: usize,
        _: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.0 = self.0.max(desired);
        Ok(true)
    }
    fn table_growing(&mut self, _: usize, _: usize, _: Option<usize>) -> wasmtime::Result<bool> {
        Ok(true)
    }
}

impl wasmi::ResourceLimiter for Peak {
    fn memory_growing(
        &mut self,
        _: usize,
        desired: usize,
        _: Option<usize>,
    ) -> Result<bool, wasmi::errors::LimiterError> {
        self.0 = self.0.max(desired);
        Ok(true)
    }
    fn table_growing(
        &mut self,
        _: usize,
        _: usize,
        _: Option<usize>,
    ) -> Result<bool, wasmi::errors::LimiterError> {
        Ok(true)
    }
    fn instances(&self) -> usize {
        16
    }
    fn tables(&self) -> usize {
        16
    }
    fn memories(&self) -> usize {
        16
    }
}

struct Host {
    ctx: wasmtime_wasi::WasiCtx,
    table: wasmtime::component::ResourceTable,
    peak: Peak,
}

impl wasmtime_wasi::WasiView for Host {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

fn peak_rss() -> u64 {
    // SAFETY: getrusage writes only the struct it is given.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let max = usage.ru_maxrss as u64;
    // Bytes on macOS, KiB on Linux.
    if cfg!(target_os = "macos") {
        max
    } else {
        max * 1024
    }
}

fn engine(pulley: bool) -> wasmtime::Result<wasmtime::Engine> {
    let mut config = wasmtime::Config::new();
    config.cranelift_opt_level(wasmtime::OptLevel::Speed);
    // Wasmtime's own cap on native stack for wasm frames is 512 KiB, and
    // Blitz's recursive layout passes it on `deep`; 8 MiB, as a native main
    // thread has. `main` runs everything on a 32 MiB thread to hold it.
    config.max_wasm_stack(WASM_STACK);
    // wasmtime-wasi turns on async support, whose fiber stack must hold it.
    config.async_stack_size(WASM_STACK + (1 << 20));
    if pulley {
        config.target("pulley64")?;
    }
    wasmtime::Engine::new(&config)
}

/// CI's half: the component compiled ahead of time to `out`.
fn compile(pulley: bool, wasm: &str, out: &str) -> wasmtime::Result<String> {
    let engine = engine(pulley)?;
    let bytes = std::fs::read(wasm)?;
    let t = Instant::now();
    let cwasm = engine.precompile_component(&bytes)?;
    let compile_ms = t.elapsed().as_millis();
    std::fs::write(out, &cwasm)?;
    Ok(format!(
        "cwasm_bytes={} compile_ms={compile_ms}",
        cwasm.len()
    ))
}

/// The person's half: a `.cwasm` loaded, instantiated and run.
fn wasmtime_run(pulley: bool, cwasm: &str, args: &[String]) -> wasmtime::Result<String> {
    use wasmtime::component::{Component, Linker};
    let engine = engine(pulley)?;
    let t = Instant::now();
    // SAFETY: the file was made by `compile` with the same engine settings.
    let component = unsafe { Component::deserialize_file(&engine, cwasm)? };
    let load_us = t.elapsed().as_micros();
    let mut linker = Linker::<Host>::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
    let mut builder = wasmtime_wasi::WasiCtx::builder();
    let mut argv = vec![String::from("s14-engine")];
    argv.extend_from_slice(args);
    builder.inherit_stdio().args(&argv);
    // S14_RAW=<dir>/<file>: the guest writes its last frame there (for pixel
    // diffs against native), through a preopen of <dir> as `/out`.
    if let Some(raw) = std::env::var_os("S14_RAW").map(std::path::PathBuf::from) {
        let dir = raw
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(std::path::Path::new("."));
        builder.preopened_dir(dir, "/out", wasmtime_wasi::FsPerms::ReadWrite)?;
        let name = raw
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        builder.env("S14_RAW", format!("/out/{name}"));
    }
    let mut store = wasmtime::Store::new(
        &engine,
        Host {
            ctx: builder.build(),
            table: wasmtime::component::ResourceTable::new(),
            peak: Peak::default(),
        },
    );
    store.limiter(|h| &mut h.peak);
    let t = Instant::now();
    let command =
        wasmtime_wasi::p2::bindings::sync::Command::instantiate(&mut store, &component, &linker)?;
    let instantiate_us = t.elapsed().as_micros();
    let t = Instant::now();
    let ok = command.wasi_cli_run().call_run(&mut store)?.is_ok();
    let run_ms = t.elapsed().as_millis();
    Ok(format!(
        "ok={ok} load_us={load_us} instantiate_us={instantiate_us} run_ms={run_ms} peak_guest_mib={}",
        store.data().peak.0 >> 20
    ))
}

struct WasmiHost {
    wasi: wasmi_wasi::WasiCtx,
    peak: Peak,
}

fn wasmi_run(wasm: &str, args: &[String]) -> Result<String, Box<dyn std::error::Error>> {
    let engine = wasmi::Engine::default();
    let bytes = std::fs::read(wasm)?;
    let t = Instant::now();
    let module = wasmi::Module::new(&engine, &bytes[..])?;
    let load_us = t.elapsed().as_micros();
    let mut linker = wasmi::Linker::<WasmiHost>::new(&engine);
    wasmi_wasi::add_to_linker(&mut linker, |h| &mut h.wasi)?;
    let mut argv = vec![String::from("s14-engine")];
    argv.extend_from_slice(args);
    let wasi = wasmi_wasi::WasiCtxBuilder::new()
        .inherit_stdio()
        .args(&argv)?
        .build();
    let mut store = wasmi::Store::new(
        &engine,
        WasmiHost {
            wasi,
            peak: Peak::default(),
        },
    );
    store.limiter(|h| &mut h.peak);
    let t = Instant::now();
    let instance = linker.instantiate_and_start(&mut store, &module)?;
    let instantiate_us = t.elapsed().as_micros();
    let start = instance.get_typed_func::<(), ()>(&store, "_start")?;
    let t = Instant::now();
    let ok = match start.call(&mut store, ()) {
        Ok(()) => true,
        // `proc_exit(0)` ends `_start` with an exit "error".
        Err(e) => e.i32_exit_status() == Some(0),
    };
    let run_ms = t.elapsed().as_millis();
    Ok(format!(
        "ok={ok} module_bytes={} load_us={load_us} instantiate_us={instantiate_us} run_ms={run_ms} peak_guest_mib={}",
        bytes.len(),
        store.data().peak.0 >> 20
    ))
}

/// Native stack Wasmtime lets wasm frames use.
const WASM_STACK: usize = 8 << 20;

fn main() {
    let runner = std::thread::Builder::new()
        .stack_size(4 * WASM_STACK)
        .spawn(run)
        .expect("a thread for the guest");
    if runner.join().is_err() {
        std::process::exit(1);
    }
}

fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(mode), Some(wasm)) = (args.first(), args.get(1)) else {
        eprintln!(
            "usage: s14-run compile-aot|compile-pulley <component.wasm> <out.cwasm>, or aot|pulley <cwasm> [fixture] [runs], or wasmi <module.wasm> [fixture] [runs]"
        );
        std::process::exit(2);
    };
    let rest = &args[2..];
    let line = match mode.as_str() {
        "compile-aot" | "compile-pulley" => match rest.first() {
            Some(out) => compile(mode == "compile-pulley", wasm, out).map_err(|e| e.to_string()),
            None => Err(String::from("compile needs an output path")),
        },
        "aot" => wasmtime_run(false, wasm, rest).map_err(|e| format!("{e:?}")),
        "pulley" => wasmtime_run(true, wasm, rest).map_err(|e| format!("{e:?}")),
        "wasmi" => wasmi_run(wasm, rest).map_err(|e| e.to_string()),
        other => Err(format!("no mode {other}")),
    };
    match line {
        Ok(line) => println!(
            "s14-host mode={mode} {line} peak_rss_mib={}",
            peak_rss() >> 20
        ),
        Err(e) => {
            // One line, so the report can say which run failed and why.
            let first = e
                .lines()
                .find(|l| l.contains("trap") || l.contains("stack") || l.starts_with("Caused"))
                .or_else(|| e.lines().next())
                .unwrap_or("")
                .trim()
                .replace(' ', "_");
            println!("s14-host mode={mode} error={first}");
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
