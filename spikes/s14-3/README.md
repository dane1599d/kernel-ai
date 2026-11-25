# Spike S14-3 js-engines

P6 groundwork G2. This spike asks which JavaScript engine a browser engine running inside Wasm could use. It builds two candidates for `wasm32-wasip2` and runs them on Wasmtime ahead of time:

- **Boa 0.22**: pure Rust, built with `annex-b`, without `intl`.
- **rquickjs 0.14**: the Rust bindings to QuickJS-NG. Its C code is built by the wasi-sdk 24 that `rquickjs-sys` downloads.

StarlingMonkey is left out because it is MPL-2.0. It goes in only if the founder asks.

This is a `spike/*` branch, so it is never merged (ADR 0000:43). Its workflow, `.github/workflows/spike-js-engines.yml`, runs on every push to `spike/js-engines` and can also be started by hand (workflow_dispatch). All runs are on GitHub runners.

## What runs

- **Runners** (`boa/`, `qjs/`): each implements the engine half of `common/runner.rs`: a fresh engine, `print`, `$262.evalScript`/`gc`/`global`, scripts, modules, and jobs. Each binary has three modes:
  - `empty` measures start-up.
  - `bench <files>` runs Octane.
  - `t262 <root> <manifest> <start>` runs test262.
- **Driver** (`drive.py`):
  - builds the test262 manifest from each test's front matter, including modes, includes, negative type and async;
  - feeds the manifest to the runner, and if a test hangs (no output line for 20 s) or traps, counts it as failed and restarts the runner after it;
  - summarizes the results;
  - times Octane and start-up under `/usr/bin/time`.
- **Pins**: Wasmtime v51.0.0, test262 `4db99ac4`, Octane `570ad1cc`.
- **Wasmtime limits**: 2 GiB of linear memory, a 16 MiB Wasm stack on a 64 MiB host stack. The guest's own stack is 8 MiB (`.cargo/config.toml`).

### How test262 is scored

The pass rate covers `test/built-ins`, `test/language` and `test/annexB`. `intl402` and `staging` are left out.
- **Modes:** a test without flags runs in both sloppy and strict mode, and it passes only if both pass. `onlyStrict`, `noStrict`, `raw` and `module` tests run once, in their one mode.
- **Negative tests** pass when the thrown value's constructor has the expected name. The phase is not checked; parse-phase tests start with `$DONOTEVALUATE()`.
- **Async tests** pass on `Test262:AsyncTestComplete`.
- **Missing `$262` features:** `createRealm`, `detachArrayBuffer` and `agent` are not provided, for either engine. Tests that need them fail on both alike, so the figures are lower bounds against each engine's own test262 runner.

### Other measurements

- **Octane:** Richards, DeltaBlue, Crypto and RayTrace, each run alone with `base.js`. The score is the median of 3 runs.
- **Start time:** the wall time of a whole `wasmtime run` of `empty`, median of 20. Also the engine's own init time measured inside the guest.
- **Memory:** the guest's linear memory at the end, which only grows, so it is the peak. Also the host process's peak RSS.
- **Size:** the `.wasm` file, its gzip -9 size, and the `.cwasm` Wasmtime compiled from it. Built with release, LTO, one codegen unit, stripped, `panic=abort`.

A native build of each runner, on the same runner machine, sits beside every wasm figure for comparison.

## Figures

The run, and its figures, are added below once the workflow has finished.
