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

Run [45720308618](https://github.com/dane1599d/kernel-ai/actions/runs/45720308618), at commit 9b2f8ded, on GitHub `ubuntu-24.04` runners. Each `.cwasm` was compiled on the machine that ran it.

### test262 pass rate

A test counts as one, however many modes it runs in.

| | Boa 0.22 | QuickJS-NG (rquickjs 0.14) |
|---|---|---|
| All three directories (48,803 tests) | **95.35%** | **86.88%** |
| Without `built-ins/Temporal` (44,187) | 94.88% | **95.96%** |
| `test/language` | 97.20% | 97.20% |
| `test/built-ins` | 93.57% | 76.29% |
| `test/annexB` | 94.11% | 93.46% |
| `built-ins/Temporal` (4,616) | 4,613 pass | 0 (no Temporal) |
| Runs that hung (20 s) or trapped | 26 | 35 |

Both engines fail the same groups of tests, because the harness lacks some `$262` features:
- detachArrayBuffer: about 490 runs on each engine;
- `atomicsHelper.js`: 224 runs on each engine;
- ShadowRealm: 118 runs on each engine.

Some failures are each engine's own:
- **Boa** lacks explicit resource management (`using`, `DisposableStack`), about 470 runs, and fails some module and dynamic-import cases.
- **QuickJS-NG** lacks Temporal and Atomics (348 runs), fails `import.meta` (293 runs), and fails some module-loading cases.

### Octane

The score is the median of 3; higher is better.

| Test | Boa wasm AOT | Boa native | QuickJS-NG wasm AOT | QuickJS-NG native |
|---|---|---|---|---|
| Richards | 91 | 249 | 627 | 745 |
| DeltaBlue | 82 | 221 | 614 | 739 |
| Crypto | 91 | 268 | 470 | 725 |
| RayTrace | 177 | 422 | 1124 | 1684 |

- **On Wasmtime ahead of time**, Boa keeps 34-42% of its native score, and QuickJS-NG keeps 65-84%.
- **In wasm**, QuickJS-NG scores 5.2-7.5x Boa.

### Start time, memory and size

Start time and memory are for `empty`: start the engine and run `0`. The wall time is a whole `wasmtime run`, median of 20.

| | Boa wasm | Boa native | QuickJS-NG wasm | QuickJS-NG native |
|---|---|---|---|---|
| Wall time of a run | 6.5 ms | 2.1 ms | 6.2 ms | 1.7 ms |
| Engine init, inside the guest | 0.42 ms | 0.36 ms | 0.35 ms | 0.21 ms |
| Guest linear memory | 9.3 MiB | | 8.3 MiB | |
| Peak RSS of the process | 31.8 MiB | 7.2 MiB | 21.6 MiB | 3.5 MiB |

During Octane, guest memory peaked at 9.9-13.3 MiB for Boa and 8.6-8.8 MiB for QuickJS-NG. Peak RSS was 38-42 MiB for Boa and 27 MiB for QuickJS-NG.

| Size (bytes) | Boa | QuickJS-NG |
|---|---|---|
| `.wasm` | 7,370,771 | 1,259,273 |
| `.wasm`, gzip -9 | 1,997,382 | 437,169 |
| `.cwasm` (x86-64) | 16,772,040 | 3,208,296 |
| Native binary | 8,728,320 | 1,555,472 |

### What the figures say

- **Conformance:** the two are level once Temporal is set aside. QuickJS-NG passes 95.96% and Boa 94.88%.
- **Temporal:** Boa is the only one of the two that has it. That accounts for its higher overall rate.
- **Speed:** QuickJS-NG is 5-7x faster under Wasmtime and loses less to wasm.
- **Size:** QuickJS-NG is about a sixth the size.
- **Start-up** is the same for both: well under a millisecond of engine init, and about 6 ms for a whole `wasmtime run`.

A browser engine inside a Wasm component would pay for Boa in speed and size, and would get Temporal and pure-Rust code in return. QuickJS-NG needs a C toolchain (wasi-sdk) in the build.

These figures are lower bounds against each engine's own test262 runner, because of the `$262` gaps above. No JIT is involved for either engine.
