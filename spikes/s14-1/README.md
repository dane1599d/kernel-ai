# Spike S14-1 `engine-in-wasm`

ADR 0014 (in PR #1268) asks whether Blitz, the Rust HTML/CSS engine, can run as a
Wasm component fast enough to be Kernel AI's browser engine. This spike measures
it. It lives on `spike/engine-in-wasm`, which is **never merged**.

## What it runs

- `engine/`: Blitz 0.3.0-beta.4 (blitz-html, blitz-dom with `floats`, blitz-paint)
  painting into anyrender_vello_cpu 0.18 at 1280x800, with one embedded font
  (the repo's Inter). Style runs sequentially: a WASI guest has no threads. The
  same program builds natively and for `wasm32-wasip1` and `wasm32-wasip2`, and
  times parse, style + layout, and paint itself.
- `engine/src/fixtures.rs`: ten pages made in code from a fixed seed. Nothing
  from the web is committed. `article` is the long Wikipedia-shaped page that
  the first-paint bar is about.
- `runner/`: runs the guest on:
  - Wasmtime 51 with Cranelift, compiled ahead of time to a `.cwasm`
    (`compile-aot`, then `aot`), as a CI-made, signed one would be;
  - Wasmtime's Pulley interpreter (`compile-pulley`, then `pulley`);
  - wasmi 2.0.3, the interpreter apps run on today (`wasmi`, the scalar
    wasip1 module).

  It records peak guest linear memory through each runtime's resource limiter.

## Modes in the report

| Mode | Build | Runs on |
|---|---|---|
| native | host release build | the host CPU |
| aot | wasip2 component with `simd128` | Wasmtime, Cranelift, ahead of time |
| aot-scalar | wasip2 component, no SIMD | Wasmtime, Cranelift, ahead of time |
| pulley | wasip2 component with `simd128` | Wasmtime's Pulley interpreter |
| wasmi | wasip1 module, no SIMD | wasmi 2.0.3 (its `simd` feature off, as in wasmhost) |

Every wasm build gets an 8 MiB stack (`.cargo/config.toml`). With Rust's default
1 MiB, Blitz's recursive layout traps on `deep` (200 nested blocks) in every runtime.

## Bars (ADR 0014)

- Ahead-of-time code at most 2x native.
- `article` to first paint in at most 400 ms, measured as a fresh process's
  load + instantiate + first render.
- Peak at most 256 MiB.

wasmi's and Pulley's figures are recorded, not judged.

## Run it

```bash
./build.sh            # CARGO=... to use another cargo
RUNS=5 ./measure.sh   # writes out/raw.txt and out/report.md
```

CI runs both scripts in `.github/workflows/s14-1-engine.yml` on every push to this
branch, or by hand with `workflow_dispatch`. The report goes to the job summary
and, with the raw lines, to a 30-day artifact.

## Licences

This workspace has its own Cargo.lock, so none of this enters main's.
- Blitz, anyrender, vello_cpu, Wasmtime and wasmi are MIT or Apache-2.0.
- Stylo, selectors and cssparser are MPL-2.0. ADR 0009 D4 makes MPL "ask" on
  main (founder question 2 in PR #1268); a spike branch may use them.
- Inter is SIL OFL 1.1 and is already in `assets/fonts/`.
