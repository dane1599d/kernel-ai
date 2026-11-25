# S14-2 `wasmtime-nostd` (spike, branch `spike/wasmtime-nostd`, never merged)

Wasmtime 51's runtime alone, `no_std`, no compiler, in a ring-3 process of ours,
loading a signed `.cwasm` made ahead of time. Pass and fail rules are in the P6
plan; the figures come from the `wasmtime-nostd` boot case's log (KVM).

## What is here

- `src/lib.rs`: the engine configuration shared by both sides, and `run`, which
  verifies an ed25519 signature over the `.cwasm`, then deserializes, instantiates
  and calls it, timing each step.
- `../s14-2-host`: compiles `WAT` for native x86-64 (baseline CPU, no host
  features inferred) and for Pulley, signs both with a fixed test seed, and writes
  `native.cwasm`, `pulley.cwasm`, their `.sig` files and `public.key` here.
- `user/src/bin/wasmtimespike.rs` and its `platform.rs`: the ring-3 program, with
  the glue Wasmtime's `custom` platform asks of an embedder: page size, mmap,
  remap, munmap, mprotect and two TLS slots. Memory is MEM_RESERVE plus
  MEM_COMMIT; `mprotect` with EXEC calls the throwaway **MEM_EXEC** (167).
- Kernel: `MEM_EXEC` (`kernel/src/process/mem.rs`, `memory/user.rs`): makes the
  committed owned pages of a range read-execute (not writable). Throwaway, no
  capability check beyond the caller's own address space, never to be merged as is.
- `test=wasmtime-nostd` (KVM only): spawns the program, expects exit 0.

## Findings

1. **The runtime alone builds and links for our hard-float ring-3 target** with
   `-Zbuild-std`. With `signals_based_traps(false)`, no `async` and no
   `custom-virtual-memory`, the only symbols it asks for are `wasmtime_page_size`,
   `mmap_new`, `mmap_remap`, `munmap`, `mprotect` and `tls_get`/`tls_set`.
2. **The ring-3 target's name breaks target-lexicon**, which Wasmtime needs and
   whose build script parses the target's name: `x86_64-kai-user` is
   "UnrecognizedOperatingSystem". `x86_64-kai-none` parses (unknown vendor is
   fine, OS `none`). This branch renames `user/x86_64-kai-user.json`. A real
   change would pick the name once.
3. **Native code needs one kernel call**: a way to make committed pages
   read-execute. The kernel had none (heap and MEM_COMMIT pages are NX). `MEM_EXEC`
   is that call; Pulley needs none.
4. **Cross-compiling the `.cwasm` needs Wasmtime's `all-arch` feature** on the host
   tool (an aarch64 Mac cannot otherwise target x86-64). Pass `Config::target`
   with the same triple the runtime has (`x86_64-kai-none`) or the engine refuses
   the file as built for another host. `Config::target` also stops host CPU
   feature inference, so the code is baseline x86-64 (SSE2).
5. **`wat`**: Wasmtime 51's `wat` feature wants `wat` 1.263; xtask pins 1.262, so
   the host tool depends on `wat` 1.262 directly.
6. **`cargo deny`: RUSTSEC-2026-0441, `pin-utils` unmaintained, was not
   Wasmtime's.** `pin-utils` came in through `futures-util` 0.3.31 (via `js-sys`,
   `web-time`, `rhai`, `shellcore`: the wasm32 build of rhai). Main's lock has
   `futures-util` 0.3.34, which no longer depends on it. This branch downgraded
   `futures-util` to 0.3.31 by hand (`cargo update -p futures-util --precise
   0.3.31`) to get past a transient index error while adding Wasmtime, and that
   is what added `pin-utils`. Wasmtime itself does not pull it: its only
   `futures` dependency is the optional one behind `async` and
   `component-model-async`, which this crate does not enable. Fixed by putting
   `futures-util` back to 0.3.34; no feature flag is needed and `deny.toml` is
   untouched. (The lockfile still lists `futures` 0.3.31 because Cargo locks every
   optional dependency of every feature; it is not built.)
