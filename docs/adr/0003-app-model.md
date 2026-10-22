# ADR 0003: The app model and app ABI

Proposed, 2026-10-21. For the founder's approval; nothing here is decided until approved.

Repo citations are `path:line` on main at `ec5ecff`. Open PRs are cited as `branch@commit`. External facts were read at the source on 2026-10-21 unless marked unverified. Figures marked "est." are estimates; nothing here was built, run or measured.

## Context

Kernel AI already has a live-editable unit, the M9 component, but no app model.

- **The unit today.** A component is a plain core WebAssembly module, Rust built for `wasm32-unknown-unknown` with the `component` profile (Cargo.toml:55-62). It runs on wasmi 2.0 in `comphost`, one ring-3 process per component (docs/decisions.md:90, kabi/src/plan.rs:12-13). It carries `kai.manifest` and `kai.source` sections (kabi/src/live.rs:196-198).
- **The contract** `kai-lifecycle-1` is WIT on paper only, bound by hand with results packed as `(ptr << 32) | len` (kabi/src/live.rs:9-41, compsdk/src/abi.rs:65-70, wasmhost/src/instance.rs:223-237). Its only imports are `kai.log` and `kai.panic` (wasmhost/src/instance.rs:157-161).
- **No side effects, by design.** "A component has no clock, no randomness and no access to other services" (kabi/src/live.rs:43-44). Dual-run probation depends on this (livecore/src/probation.rs:1-5).
- **Requests are untyped text** answered `ok ...` or `err ...` (compsdk/src/lib.rs:54-55), at most 8,192 bytes (kabi/src/lib.rs:160). Components declare no actions. The model's script API is a hand-kept table of 7 functions (llmproto/src/script.rs:51-94).
- **Versioning is nominal.** `lifecycle` must be exactly `1` (livecore/src/artifact.rs:14, :139-140). The interface pin promises "that needs a restart" (livecore/src/host.rs:243-253), but xtask always writes `<component>@1` (xtask/src/components.rs:158), so it never trips and no restart path exists. There are no `.wit` files and no WIT crates in Cargo.lock.
- **Limits.** 16 processes (kernel/src/process/mod.rs:54), 64 threads (sched/src/lib.rs:39), 8 components (kabi/src/live.rs:192), a 64 KiB checkpoint (kabi/src/live.rs:188). A process may not call a service that holds CALL (kernel/src/process/syscall.rs:334-354), so the call graph is one hop deep. Editing is wired to notes only (editcore/src/lib.rs:46-50).
- **Speed is unmeasured.** wasmi runs the call-loop (`portable-dispatch` alone, wasmhost/Cargo.toml:18), which wasmi's author calls "very slow" and does not recommend (Wasmi 2.0 post, 2026-09-01). comphost is soft-float (docs/decisions.md:140), so wasmi's float code runs in software.
- **The syscall table** has 37 calls, numbered 0-36 (kabi/src/call.rs:4-125). PR #16 adds RESOLVE as 37 (net-dhcp-dns-b5bdb7@131aa77, kabi/src/call.rs:134).

The roadmap asks for "WebAssembly components with WIT interfaces, so an app's interface also describes its actions" (docs/roadmap.md:139). It asks that every feature declare "name, arguments, risk tier and undo" (docs/roadmap.md:127), and that the syscall ABI, the app ABI and on-disk formats be versioned from their first release (docs/roadmap.md:131). P3 needs "P0's app model with its UI API"; P4 needs every app to declare its actions (docs/roadmap.md:142-143). The founder wants the AI to operate everything, and plain requests to just happen (docs/decisions.md:201, :203, :205). Founder row docs/decisions.md:15 says "Plain WebAssembly modules on wasmi are the live-swappable unit."

This ADR decides what an app is, how it is built, versioned and isolated, how it declares actions, and how it reaches anything outside itself.

## Options considered

The judges weighed three designs (scores out of 10).

| Design | Score | In one line |
|---|---|---|
| A. Wasm components and WIT everywhere, on Wasmtime now | 6 | Standard component binaries on Wasmtime 48 LTS's no_std runtime, precompiled on the host |
| B. Native processes and our own IDL | 4 | Static ELF apps, a FIDL-like IDL `KIDL` and a CBOR wire format `KWIRE` |
| C. Core Wasm now, components later | 7 | wit-bindgen guests as canonical-ABI core modules on wasmi, our own synchronous lift and lower, Wasmtime as a test oracle |

**A, Wasmtime now (lost, 6).**
- For: the most reuse. Wasmtime's `Linker` matches semver-compatible interfaces both ways (docs.rs, wasmtime 50.0.0). It has an LTS line: every 12th monthly release is supported for 24 months (docs.wasmtime.dev/stability-release.html). Typed actions come straight from WIT. Its shadow-probation idea and its spike are kept below.
- Against: the no_std runtime cannot compile. "no_std platforms must be used in AOT mode" (docs.wasmtime.dev/stability-platform-support.html). So every app needs a host-made `.cwasm` tied to one Wasmtime build, loaded through `unsafe` deserialisation. An in-OS build (ADR [0012](0012-terminal-compiler.md)) or an outside `.wasm` cannot run without a trusted host compiler.
- Against: Pulley "will never be as fast as native Cranelift backends" (same page), and its tracking issue says it does not outrun other interpreters (wasmtime issue #10102). Speed would then need run-time executable pages, which ADRs [0006](0006-existing-software.md) and 0012 rule out before v1.0.
- Against: the platform API is "not guaranteed to be stable" (platform-support page). The work is large: about 8,000-11,000 lines by v0.1 (A's est.). R1, Wasmtime in our ring 3, is unmeasured.
- Judge's corrections. A said the OS reads sections with wasmparser 0.262.0. The OS ships wasmparser 0.228.0 through wasmi; 0.262.0 is only in Cargo.lock through the host-side `wat` dev-dependency (Cargo.lock:1190-1201). A's "ranked call rule" is superseded by ADR [0001](0001-capabilities.md)'s routes. Its dotted action names break the snake_case rule. Its Cranelift-versus-LLVM figure (1.5-1.9x) is unverified.

**B, native apps and KIDL (lost, 4).**
- For: native speed with AVX2, one process model shared with P6, a clean licence story.
- Against: we would own a language, its compiler, a code generator per language and a wire format for good. Live editing would reach only components, so native apps lose M9's probation. Binaries would be x86-only, and static libraries would be baked into every app.
- Against: it needs channels, shared memory and TLS in the kernel before the first app draws, about 11,000 lines and 4-5 months (B's est.).
- Against: it contradicts the roadmap's WIT components (docs/roadmap.md:139) and ADR 0001's decision that the public ABIs are the `kai:*` WIT packages.
- Its kernel pieces survive elsewhere: one `Shm` kind (ADR 0001), KRNG v1 rings (ADR [0007](0007-drivers.md)), `Surface` (ADR [0005](0005-graphics.md)). Its codegen base, Idol, is MPL-2.0, which is now "ask" under the founder's 2026-10-21 rule. Its seL4 IPC figure (743 cycles) is unverified.

**C, core Wasm now (won, 7, with grafts).**
- For: guests are built by upstream wit-bindgen, so each app is already the core module the canonical ABI defines. The engine can change without rebuilding apps, nothing is tied to a `.cwasm`, and in-OS compiles keep working.
- For: the toolchain requires every app to declare actions, and effects are data, so probation and the agent's tiers still hold. Every PR checks our ABI against Wasmtime. It has the cleanest licences and the smallest effort (M-L), and it keeps row :15 and M9.
- Against: we own a 1,500-2,000-line synchronous canonical-ABI subset (est.) that dies if we move to Wasmtime. The research advised against writing one. Its "all effects as events" style made storage reads awkward; the graft below makes reads synchronous.
- Judge's corrections, all applied below. The ABI stamp is ADR 0001's `kabi::ABI` (1,0), not C's `ABI_REVISION`. The subset must include resources with own and borrow (ADR 0001's handle binding). Action names are snake_case. C said only the candidate's effects are performed; in fact, on a candidate fault or a failed last check, the standby's reply is sent (livecore/src/probation.rs:38-45). The rule is therefore "the version whose reply is sent". On dispatch modes: wasmi's post says direct-threaded code is fastest, and `auto-dispatch` picks threaded code where it can. The recorded reason for `portable-dispatch` (stack growth with debug assertions, docs/decisions.md:93, wasmhost/Cargo.toml:16-17) concerns the tail-call modes, so the switch-loop is the safe first trial.

**Reuse options.** Licence classes follow ADR 0009 and the founder's 2026-10-21 rule: "free" needs no ask but is listed in each PR; "ask" needs the founder.

| Project, version (date) | Licence (class) | Outcome |
|---|---|---|
| wasmi 2.0.0 (2026-09-01) | MIT/Apache-2.0 (free) | Kept as the engine |
| wit-bindgen 0.63.0 (2026-10-14) | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT (free) | Adopted: guest bindings for Rust; C and C++ generators later |
| wit-parser, wit-component, wasm-encoder, wasmparser 0.262.0 (2026-10-13) | same triple (free) | Adopted on the host only: build service, `wit-check`, oracle wrapping |
| wasm-wave 0.262.0 (`#![no_std]`, src/lib.rs:7); its logos 0.16.1 and thiserror 2 | same triple; MIT OR Apache-2.0 (free) | Adopted in the OS: call and log value text |
| Wasmtime 50.0.0 (2026-10-20); 48.0.7 LTS (2026-10-19) | Apache-2.0 WITH LLVM-exception (free) | Dev-dependency oracle now; second engine only on a trigger |
| wasm_component_layer 0.1.18 (2024-08-02) | Apache-2.0 (free) | Not adopted: no release since; pins wasmtime-environ 18.0.1 and wit-parser 0.13.0. wasmi issue #1665, which points to it, was closed on 2025-09-23. Fork waclay 0.2.2 (2025-12-10) not reviewed. |
| Hyperlight-wasm 0.15.0 (2026-09-04) | Apache-2.0 (free) | Reference only: a Wasmtime platform shim of about 370 lines of Rust (src/hyperlight_wasm_runtime/src/platform.rs), if the backend comes |
| Wassette `component2json` 0.8.1 (2026-10-19) | MIT (free) | Not adopted here; ADR [0010](0010-skill-pack.md) takes ideas |
| wasmi-benchmarks (pushed 2026-09-01) | MIT and Apache-2.0 (free) | Spike harness |
| talc 5.1.1; minicbor 2.3.0 | MIT (free); BlueOak-1.0.0 (proposed free, ADR [0009](0009-licence.md)) | Kept in guests: allocator; state |
| Idol (pushed 2026-10-10); mlibc 7.0.0 (2026-08-02) | MPL-2.0 (ask); MIT (free) | B's choices; not adopted here |
| FIDL, Fuchsia ABI stamps, Android stable AIDL, App Intents, AppFunctions | various | Ideas only: versioning, freeze checks, build-time action metadata |

## Decision

Every app is a small WebAssembly program that runs sealed in its own process, on the wasmi engine Kernel AI already uses. Each app lists what it can do in WIT, a standard interface language, and the build turns that list into actions the AI can call with the same tiers, log and undo as everything else. An app never reaches anything outside itself directly: it asks, and the agent core runs the request at once, asks you first or refuses it, by its risk tier. A faster engine, Wasmtime, can be added later behind the same interface without rebuilding any app.

In detail: base design C. Grafts: A's per-app `kai.actions` extraction, semver linking and freeze rule, synchronous reads and its Wasmtime spike; ADR 0001's ABI stamp and resources. Not B's IDL.

### 1. The unit

- **An app** is a WIT world built by upstream wit-bindgen 0.63.0 into a core module for `wasm32-unknown-unknown` with the `component` profile. The crate is used with `default-features = false` and features `macros` and `realloc`, so guests stay no_std and synchronous. The module is the canonical ABI, unwrapped: no component binary layer.
- **Apps and live components are one thing.** Notes becomes the first app. A system component without UI uses the smaller `component` world (section 3).
- **One ring-3 `comphost` process per app.** There are two instances only during probation, and apps never share a process. The reason is Spectre: V8 concluded that the only effective mitigation is to keep sensitive data out of the process (v8.dev/blog/spectre, 2019-04-23).
- **Linux apps are a foreign class** outside `kai:app`: they run in ADR [0006](0006-existing-software.md)'s micro-VM and show, in v0.2, as one foreign `Surface` (ADR 0005).
- **Apps are Wasm only.** Native code is for system services, drivers and the Linux programs inside P6's micro-VM (founder question 4).
- **The artifact** is the module plus custom sections, read by livecore's existing section reader. That reader requires the core-module header (livecore/src/artifact.rs:10, :44), which apps keep, so no second wasmparser ships. This ADR owns the artifact and its list of sections; each section's format has the owner below.

| Section | Format | Format owner | Holds |
|---|---|---|---|
| `kai.manifest` | `kai-manifest 2` (text) | this ADR | runtime and lifecycle fields |
| `kai.source` | `kai-source 1` (unchanged, livecore/src/artifact.rs:156) | this ADR | source bundle, now with the app's `wit/` files |
| `kai.actions` | `kai-actions 1` (text) | this ADR | action rows and signatures |
| `kai.app` | `kai-app 1` (text) | ADR 0001 | capability routes |
| `kai.notices` | text | ADR 0009 | third-party notices |

- **`kai-manifest 2`.** Keys in this order, each exactly once:
  - `lifecycle 2`;
  - `component <name>`;
  - `version <label>`;
  - `world <pkg>/<world>@<semver>` (replaces `interface`; today's keys are livecore/src/artifact.rs:102-110);
  - `engine wasmi`;
  - `features <list or ->`;
  - `licence <SPDX expression>` (ADR 0009's request);
  - `build <sha256>`;
  - `source <sha256>`.

  The host reads `kai-manifest 1` and `lifecycle 1` for one release, then drops them. There is no JSON manifest. A package (ADR [0011](0011-packages.md)) wraps this artifact in `kai-package 1`.
- **App names** follow ADR 0011: `[a-z][a-z0-9-]{1,11}`, at most 12 bytes.

### 2. The engine: wasmi plus `appabi`

- **`appabi`** is a new no_std, host-tested crate with no wasmi types; wasmhost stays the only crate that names wasmi. It lifts and lowers the synchronous canonical ABI over a `GuestMemory` trait (read, write, `cabi_realloc`), and calls `cabi_post_*` after a result is copied out. It flattens by the spec's limits, `MAX_FLAT_PARAMS` 16 and `MAX_FLAT_RESULTS` 1 (CanonicalABI.md).
- **Types covered:** bool, all integers, f32, f64, char, string (UTF-8), list, record, tuple, variant, enum, option, result, flags, and **resources with own and borrow**.
  - A resource is backed by an ADR 0001 kernel handle held by comphost.
  - `own` is a handle transfer and `borrow` a lease for the call.
  - Not covered: `async func`, stream, future and error-context.
- **Linking.** wasmhost walks the module's imports, parses each `ns:pkg/iface@ver` and binds the host table for its canonical version: `0.1` while the package is 0.x, then the major. Otherwise it fails with `Reason::Link`, when the app is staged and never later.
  - Host tables for `kai:*` are generated from `wit/` by xtask with wit-parser (host only) and checked for drift in CI.
  - The OS never decodes WIT binaries. App-specific exports are typed from `kai.actions` (section 5).
- **`livecore::Instance` gains `call(func, &[Val]) -> Result<Vec<Val>, Fault>`** (today's trait is livecore/src/lib.rs:53-68). `handle` goes when `lifecycle 1` goes.
- **The oracle test, on every PR.** The same guest bytes are wrapped by wit-component 0.262.0 and run on Wasmtime 50's component runtime, a host dev-dependency that never ships. They are also run on wasmi with `appabi`. Results are compared as WAVE text over generated values, and the required result is 0 differences.
- **A second backend, only on a trigger.** Wasmtime's no_std runtime (LTS 48 line, features `runtime`, `component-model`, `pulley`) becomes a second `Instance` backend, chosen per app by `engine wasmtime`. It is adopted only when:
  - an S03-1 threshold fails (see Spikes); or
  - an app needs `async`, streams or futures; or
  - a wanted language's toolchain emits only components (JavaScript through jco, Python through componentize-py).

  Before v1.0 it runs Pulley bytecode only, because ADRs 0006 and 0012 allow no executable memory created at run time before v1.0. The `.wasm` stays the truth and a host-made `.cwasm` is a cache, accepted only from the build service or a signed package.

### 3. WIT packages and versions

- **In-tree under `wit/`:**
  - `kai:live@0.1.0`: the lifecycle interface, renamed off WIT keywords. Functions: `schema`, `init`, `migrate(state, from-schema)`, `save(to-schema) -> option<list<u8>>`, `health`, `self-test`.
  - `kai:app@0.1.0`, with these interfaces:
    - `types`: `rect { x, y, width, height: u32 }`, the fields of `gfxcore::Rect` (PR #18); `type path = string`; `type host = string`;
    - `log`: write and panic;
    - `clock`: `now-ms` and `seed`, fixed per call;
    - `storage`: `get`, `put` and `delete` on the app's own scope;
    - `effects`: `request(e: effect) -> u32`, which returns a ticket;
    - `events`: `on-event`, with `started`, `effect-done(ticket, result<string, string>)`, and input from ADR 0005.
  - `kai:app` has two worlds:
    - `component` imports `log` and exports `kai:live/lifecycle`;
    - `app` includes `component`, imports `clock`, `storage`, `effects` and `kai:ui`, and exports `events`.
  - `kai:ui@0.1.0`: its body is ADR 0005's. Apps hand a retained view tree to the native `uikit`, and no pixels enter Wasm memory.
  - Later and additive: `kai:fs` mirroring `kai-fs 1` (ADR [0004](0004-storage.md)), and `kai:net` behind a per-app grant (P5).
  - `app:<name>@<semver>`, one per app, with its `actions` interface and a world that includes `kai:app/app`.
- **Additions** carry `@since(version = ...)` (WIT.md). A guest built against a newer minor fails at staging on an older host, with `Reason::Link` naming the missing function.
- **Freeze.** `cargo xtask wit-check` compares `wit/frozen/<pkg>@<ver>.wit` with the resolved text on every PR. It refuses anything but an `@since`-gated addition, which is stable AIDL's rule. `kai:app` stays 0.x and is frozen per release until v1.0 (founder question 3).
- **Live swaps.** The exact interface pin becomes a compatibility check. The candidate must export every function the active version exports, with the same type, and may add more. A compatible world swaps live, so adding an action is a live edit. A new major restarts the app with migrated state; this gives the restart that livecore/src/host.rs:249 promises a real path.
- **The syscall ABI stays private** under ADR 0001's `kabi::ABI` (1,0), the `PT_NOTE` "KAI" and ABI_INFO. Apps never make syscalls; Wasm has no syscall instruction.

### 4. Effects: one transaction per call

- **Reads are synchronous imports** answered from the pre-call snapshot: `clock`, and `get` from `storage`. Time and a seed are fixed per call. Two versions given the same call therefore compute the same thing, and M9's "no clock, no randomness" rule still holds.
- **Writes to the app's own scope** are buffered in the call and commit with that version's checkpoint.
- **Anything outside the app is a queued effect**: another app's action, a URL, the network, files.
  - The effect type is `variant effect { call(action-call), notify(string) }`, where `action-call` is `{ name: string, args: string }`: a registry name and a WAVE record. Opening an app or a site is a `call` to a registry row such as `ui_open` or `browser_open`, so every effect meets the same tiers.
  - The queue returns in the reply to the agent core (ADR [0002](0002-agent-actions.md)), which applies the tier, logs the effect and records its undo.
  - Arguments an app supplies carry the `data` provenance label (ADR 0002), so app text alone never makes an outbound, destructive or undo-less effect run without asking.
  - Results come back as an `effect-done` event.
- **Probation.** Outside effects are performed once, from the version whose reply is sent: the candidate normally, the standby on a candidate fault or a failed last check (livecore/src/probation.rs:38-45).
  - Own-scope writes follow the checkpoint, which during probation is the standby's. This is the ledger's invariant (livecore/src/ledger.rs:6-7).
  - A difference between the two versions' effects or writes counts as a regression, as replies do today (livecore/src/probation.rs:35-37).
- **Undo of own state is free for authors.** comphost keeps each app's last 16 checkpoints in its own memory: at most 1 MiB at the 64 KiB cap. An action that changes only own state gets the inverse `Snapshot`. `undo` restores the checkpoint before the call if the state still equals the one after it (ADR 0002's compare-and-set), and asks otherwise. From P2, older history moves to ADR 0004 snapshots.
- **Outside the 8 KiB message.** comphost holds no CALL bit; the agent and `scriptrun` reach it through ADR 0001's routed `Service` handles. Payloads over 8,192 bytes go through an ADR 0001 `Shm` handle, never the message.

### 5. Actions

- **Every app world must export `actions`.** It may be empty; the build service refuses a world without it.
- **Doc comments carry the metadata:**
  - the first `///` line is the description (at most 60 bytes, ADR 0002's `doc`);
  - a `/// kai:` line gives the facts (`read-only`, `destructive`, `idempotent`, `open-world`, each `yes` or `no`; one left out takes ADR 0002's pessimistic default), `inverse=`, `cues=`, argument bounds (`max=`, `min=`), roles (`target`, `payload`, `option`) and the `from-request` flag.
- **App facts can only make a tier stricter.** The tier itself is the OS's (ADR 0002).
- **Argument types are ADR 0002's set only:**
  - WIT `enum` and `bool` become Enum;
  - integers with `min=` and `max=` become Int;
  - `string` with `max=` becomes Text;
  - `kai:app/types.path` becomes Path and `.host` becomes Host;
  - lists and options of these are allowed.

  Results may use any type in the subset. A result is labelled `data` (ADR 0002).
- **Names.** The registry name is `<app>_<function>` in snake_case: the app name with `-` as `_`, then the WIT function name with `-` as `_`. It is at most 24 bytes, checked at build. Rhai calls that same name, for example `notes_add(...)`, never a module path such as `notes::add` and never a dotted name (ADR 0002). The model's tagged call is `{"name":"notes_add","arguments":{"text":"buy milk"}}`.
- **`kai-actions 1`** is written by the build service from the WIT with wit-parser, checked against the module's `component-type` section, and read in the OS by a line parser like the manifest's. Example:

```text
kai-actions 1
world app:notes/notes@1.0.0
action notes_add
export app:notes/actions@1.0.0#add
doc Add a note and return its id.
sig func(text: string) -> result<u32, string>
arg text text max=2000 role=payload from-request
facts read-only=no destructive=no idempotent=no open-world=no
inverse snapshot
end
```

  `sig` and any `type` lines use the WIT syntax of the subset; `appabi` types app exports from them. The rows feed ADR 0002's `agentabi` registry with owner `App(id)`, the id being the app's name, and ADR 0010's app groups.
- **Calls on the wire.** A call to comphost is `kai-call 1`, then `func <export>` and `args <WAVE record>`. The reply is `kai-reply 1`, then `result <WAVE value>` and zero or more `effect ...` lines. `ctl ` requests stay as they are (kabi/src/live.rs:199-200). The action log and `show` print values as WAVE text.
- **Every UI node is an action.** A UI node activates an action call (ADR 0005), so anything the user can click, the AI can call, under the same tier, log and undo.

### 6. Speed, limits and languages

- **comphost moves to the hard-float target** (amends docs/decisions.md:140). The wasmi dispatch mode becomes S03-1's best mode that is safe with debug assertions. Text shaping, raster, crypto, compression and the model stay native host functions. Each call and each event gets its own fuel: 5 M as today (livecore/src/settings.rs:36), tunable per app.
- **This ADR owns the table limits.** `MAX_PROCESSES` goes from 16 to 64, `MAX_THREADS` from 64 to 256 and `MAX_COMPONENTS` from 8 to 32 (sizes from designs A and C, est.). They rise in the first PR that needs them: P1's `scriptrun` or P3's apps. ADR 0001's spike S01-1 (measure M5) reports the kernel memory cost. Each thread slot also has a 4 KiB vector-state area in .bss (docs/decisions.md:138), so 256 slots take 1 MiB.
- **Languages.** Rust is the v0.1 language. C and C++ are freestanding through `wit-bindgen c` and `cpp`, and in the OS through ADR 0012's `cc`. Other languages come after the Wasmtime backend.

## Consequences

- Every app is live-editable with probation and rollback, including apps with effects. Undo of app state needs no app code.
- The toolchain enforces "every app declares its actions". The registry and the prompt pack are generated from `kai.actions`, not kept by hand.
- Apps are validated bytecode with fuel and memory limits. There are no executable pages, no `unsafe` loading of native code, and no binary tied to an engine build.
- The ABI that apps see is the standard canonical ABI from the first app. An engine change needs no app rebuilt, if S03-1's wrapping check passes.
- **Costs.** We own `appabi` (est. 1,500-2,000 lines plus resources). There are no async apps in v0.1. wasmi is reported at 2.5-5.2x slower than baseline JITs and about 10x slower than native code (secondary sources, docs/research/live-editable-system.md:54; not measured by us), so heavy work must stay native. CI compiles Wasmtime, and the time this costs is unmeasured.
- **Probation is still blind to whether an effect is right.** A candidate that queues a different effect is logged as a regression, not rolled back, unless it faults.
- **Rows amended when this is approved:**
  - docs/decisions.md:15 (founder's, question 1);
  - :90, where `kai-lifecycle-1` and "kai.log the only import" give way to `kai:live` and `lifecycle 2`;
  - :93, the dispatch mode after S03-1;
  - :140, comphost becomes hard-float;
  - docs/research/live-editable-system.md:84 ("Wasmtime AOT only after a component is measured too slow") is widened: Wasmtime comes on a measured need or on one of the two other triggers in section 2.

## What it costs to change later

| Change | Cost | Why |
|---|---|---|
| wasmi to Wasmtime components | M-L | A new `Instance` backend, host tables rebound, modules wrapped by wit-component. Guests, state, `kai.actions` and actions are unchanged. `appabi` stays for wasmi apps or is deleted. |
| Async apps | Cheap before v1.0, dear after | A new `kai:app` minor or major world, with old apps on the queue model through a host adapter |
| Away from WIT | Expensive from P4 | Every app's actions and UI are WIT |
| Away from effects as data | Most binding | Apps would call services directly, which ends dual-run probation for them |
| Third-party native apps | Large but additive | A second app class with its own process model, B's channels and an SDK |
| `kai-manifest`, `kai-actions` formats | Cheap | Versioned text headers; the reader accepts N and N+1 for a release |
| Syscall ABI | Cheap | Private and stamped (ADR 0001) |
| After `kai:app@1.0.0` freezes | Two majors served side by side | Grows with every outside app |

## Spikes that settle it

No model runs in any spike, on any machine.

**S03-1 "same bytes, two engines"**, a 2-week throwaway branch. ADR 0001 refers to it as S03-1.

What to build:
1. `kai:probe@0.1.0`: 24 functions covering the subset, nested two levels, plus one resource with own and borrow.
2. A wit-bindgen guest.
3. A spike `appabi` on wasmi 2.0.0, about 800 lines (est.).
4. The same bytes wrapped by wit-component 0.262.0 and run on Wasmtime 50.0.0.
5. A notes-like guest with 1,000 notes that rebuilds a 300-node view per event.
6. A's `layout` pass: 240x67 cells, 8 KiB of wrapped text (the open question at docs/research/live-editable-system.md:238).
7. The per-call transaction, with injected candidate faults.

Where it runs: host tests on a GitHub Actions ubuntu runner; a ring-3 comphost boot case under KVM in GitHub Actions (median of 5 boots, HPET); TCG in WSL2 for correctness only.

| Measure | Pass | Fail, and what follows |
|---|---|---|
| Oracle differences over 10,000 generated calls | 0 | Any left after one more week: the Wasmtime backend comes into v0.1, if S03-2 passes |
| wit-component wraps the unmodified module | yes | no: "no guest rebuild" fails; re-judge A against C |
| Guest size against hand-bound notes (about 72 KiB, kabi/src/live.rs:203) | at most +20 KiB | over +40 KiB |
| 1 KiB string in and out, against `kai_handle` | at most 1.5x | over 3x |
| View rebuild plus lift, p99, best wasmi mode, hard float | at most 4 ms | over 16.7 ms: the Wasmtime backend comes into v0.1. From 4 to 16.7 ms: keep wasmi, move the heavy part native, re-measure in P3 |
| `layout` pass, best wasmi mode | at most 4 ms | over 8 ms |
| Effects performed twice, or by the version whose reply was not sent; own writes not from the checkpoint's version. Over 1,000 calls with 10% injected candidate faults | 0 | any: fix the transaction before P1 |

The spike also records the call-loop, the switch-loop and indirect-threaded dispatch, with soft and hard float. These are reported as relative figures (docs/decisions.md:198).

**S03-2 "Wasmtime in our ring 3"**, 1 week, on the same runners.
- Build `apphost-spike` for `user/x86_64-kai-user.json` with wasmtime 48.0.7 (`runtime`, `component-model`, `pulley`) and TLS as a static. Run the notes-like guest from a Pulley `.cwasm` precompiled on the host.
- Pass: it builds with at most 200 lines of patches and no fork; the stripped ELF is at most 4 MiB; instantiation takes at most 20 ms; fuel stops a loop; Pulley reaches at least 0.8x wasmi's best mode.
- It does not decide the base. It decides whether the backend is ready when a trigger fires.

## Phases that depend on it

| Phase | Needs from this ADR |
|---|---|
| P1 | `wit/` packages and `wit-check`; `appabi`; `lifecycle 2` and `kai-manifest 2`; compsdk on wit-bindgen; notes as `app:notes@1.0.0`; `kai.actions` into ADR 0002's registry; the effect transaction and checkpoint undo; live edits and rollback as actions; hard-float comphost |
| P2 | `storage` on kaifs (ADR 0004); checkpoint history on snapshots; a `kai:fs` mirror |
| P3 | `kai:ui@0.1.0` and input events (ADR 0005); `uikit` in comphost; raised limits; fuel per event |
| P4 | Every built-in app on `kai:app`; the edit loop beyond notes; apps as signed packages (ADR 0011); in-OS C builds (ADR 0012); `kai:app` frozen per release |
| P5 | Network as effects, or a `kai:net` import under per-app grants |
| P6 | The boundary: Linux apps stay outside `kai:app`, in ADR 0006's micro-VM; the compositor serves both |
| P7 | Nothing directly; input reaches apps only as compositor events |
| P8 | `kai:app@1.0.0` frozen (if chosen); fuzzing of `appabi`, the section readers and WAVE parsing; a security review of comphost |

## Changes to recently merged and open work

- **#15 seamless actions (merged in `0c84757`, branch tip `0a03411`).** A follow-up PR, under 30 lines (est.):
  - `shellcore/src/tiers.rs:55`: `of_script` becomes `fn of_script<'a>(table: &[(&str, Tier)], functions: &[&'a str]) -> (Tier, Vec<&'a str>)`;
  - `shellcore/src/actions.rs:26`: `Pending::asking` becomes `Vec<String>`;
  - `actions.rs:52-53` and `:68`: `tier_of` and `propose` carry `Vec<String>`;
  - `TIERS` stays keyed by plain strings (tiers.rs:35).

  Why: app action names arrive at run time from `kai.actions` and are not `'static`. If this is not done now, P1's registry must do it. ADR 0002 lists its own follow-ups to #15.
- **#16 DHCP/DNS (open, net-dhcp-dns-b5bdb7@131aa77).** No change. RESOLVE (37) stays a system call apps never see. Apps reach names through effects or a later `kai:net` import with a per-app grant.
- **#18 virtio-gpu (open, virtio-gpu-a5daaa@af7635d).** No change. Keep `gfxcore::Rect`'s `u32` fields (gfxcore/src/rect.rs:5-10 on that branch), and keep gfxcore std-free and host-tested; `kai:app/types.rect` mirrors it.
- **#19 USB input (open, usb-input-83eb7c@0717caa).** No change. Input reaches apps only as compositor events.
- **#17 Ember terminal (merged, `ec5ecff`).** No change. `termcore` must stay free of kernel and x86 code (its only dependency is pc-keyboard, termcore/Cargo.toml:14), so P4's terminal app can build it for wasm32. The theme's move into ADR 0005's `look` crate is ADR 0005's.
- **#13 parser hardening (merged).** No change now. Each PR that adds the `kai-manifest 2`, `kai-actions 1` or `kai-call 1` readers, `appabi` lifting or WAVE parsing adds hostile tests in its pattern (testkit/hostile.rs, `*/tests/hostile.rs`).
- **#14 decode speed (merged).** No effect.

## Open questions for the founder

1. Row docs/decisions.md:15 is yours. It would read: "Apps and live components are WIT worlds compiled to canonical-ABI core modules on wasmi; Wasmtime may join later behind the same ABI." Approve?
2. Will people outside the project write apps for v1.0, and must they install them without our servers? This sets when `kai:app` freezes, and rules out any design that needs a host-made binary per OS version. (ADR 0001 asks the same.)
3. What do we promise app authors: freeze `kai:app@1.0.0` at v0.1, or keep 0.x, frozen per release, until v1.0 (recommended)?
4. Apps are Wasm only; native code is for system services, drivers and the Linux programs inside P6's micro-VM. Is that acceptable, or must third-party native apps exist?
5. Which app languages matter after Rust: C, Python, JavaScript? Python or JavaScript would pull the Wasmtime backend forward.
6. Every app's state code uses minicbor 2.3.0, under BlueOak-1.0.0. ADR 0009 asks whether BlueOak joins the free class. If not, compsdk gets a small CBOR writer of our own (size unmeasured).

Crates this ADR adds, all in the free class (listed in each PR, no ask): wit-bindgen 0.63.0; wit-parser, wit-component, wasm-encoder and wasmparser 0.262.0 (host only); wasm-wave 0.262.0 with logos 0.16.1 and thiserror 2; wasmtime 50.0.0 (dev-dependency); wasmtime 48.0.7 (spike branch only). The PRs list the full transitive set.

## Sources

External, read 2026-10-21 unless noted:
- Wasmtime platform support and no_std features: https://docs.wasmtime.dev/stability-platform-support.html
- Wasmtime release and LTS policy: https://docs.wasmtime.dev/stability-release.html
- wasmtime on crates.io: 48.0.0 (2026-08-20), 48.0.7 (2026-10-19), 50.0.0 (2026-10-20): https://crates.io/crates/wasmtime
- `Linker` semver matching: https://docs.rs/wasmtime/50.0.0/wasmtime/component/struct.Linker.html
- `Module::deserialize` is `unsafe`: https://docs.rs/wasmtime/50.0.0/wasmtime/struct.Module.html
- Pulley tracking issue: https://github.com/bytecodealliance/wasmtime/issues/10102
- wit-bindgen v0.63.0, crates/guest-rust/Cargo.toml (features): https://github.com/bytecodealliance/wit-bindgen/tree/v0.63.0
- wasm-tools v1.262.0 (crates 0.262.0, 2026-10-13): wasm-wave src/lib.rs:7, wasmparser src/lib.rs:30, wit-component validation.rs: https://github.com/bytecodealliance/wasm-tools/tree/v1.262.0
- Canonical ABI and flattening limits: https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
- WIT, gates and doc comments: https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md
- Canonical interface names: https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md
- Wasmi 2.0 announcement (2026-09-01): https://wasmi-labs.github.io/blog/posts/wasmi-v2.0/
- wasmi and the component model (closed 2025-09-23): https://github.com/wasmi-labs/wasmi/issues/1665
- wasm_component_layer 0.1.18 and its dependencies: https://crates.io/crates/wasm_component_layer
- Hyperlight-wasm 0.15.0, src/hyperlight_wasm_runtime/src/platform.rs: https://github.com/hyperlight-dev/hyperlight-wasm/tree/v0.15.0
- Wassette 0.8.1: https://github.com/microsoft/wassette
- wasmi-benchmarks: https://github.com/wasmi-labs/wasmi-benchmarks
- WASI roadmap (0.3.0 on 2026-06-11): https://wasi.dev/roadmap
- V8 on Spectre (2019-04-23): https://v8.dev/blog/spectre
- Wasmtime security: https://docs.wasmtime.dev/security.html
- Android stable AIDL (updated 2026-10-09): https://source.android.com/docs/core/architecture/aidl/stable-aidl
- Fuchsia compatibility (updated 2026-10-12): https://fuchsia.dev/docs/concepts/versioning/compatibility
- Idol: https://github.com/oxidecomputer/idolatry
- mlibc: https://github.com/managarm/mlibc

Repo, main at `ec5ecff`:
- docs/roadmap.md:127-131, :139-147
- docs/decisions.md:15, :90, :93, :138, :140, :198, :201, :203, :205
- docs/research/live-editable-system.md:54, :84, :238
- kabi/src/live.rs:9-44, :188-204
- kabi/src/lib.rs:160
- kabi/src/call.rs:4-125
- kabi/src/plan.rs:12-13
- compsdk/src/lib.rs:54-55
- compsdk/src/abi.rs:65-70
- wasmhost/Cargo.toml:16-18
- wasmhost/src/instance.rs:157-161, :223-237
- livecore/src/lib.rs:53-68
- livecore/src/artifact.rs:10, :14, :44, :102-110, :139-140, :156
- livecore/src/host.rs:243-253
- livecore/src/probation.rs:1-5, :35-45
- livecore/src/ledger.rs:6-7
- livecore/src/settings.rs:36
- xtask/src/components.rs:158
- editcore/src/lib.rs:46-50
- kernel/src/process/mod.rs:54
- kernel/src/process/syscall.rs:334-354
- sched/src/lib.rs:39
- llmproto/src/script.rs:51-94
- shellcore/src/tiers.rs:35, :55
- shellcore/src/actions.rs:26, :52-53, :68
- termcore/Cargo.toml:14
- testkit/hostile.rs
- Cargo.toml:55-62
- Cargo.lock:1190-1201

Branches: net-dhcp-dns-b5bdb7@131aa77 kabi/src/call.rs:134; virtio-gpu-a5daaa@af7635d gfxcore/src/rect.rs:5-10.

Inputs: the P0 decision sheet, the topic 03 judgement, designs A, B and C, and the topic 03 code and prior-art research (2026-10-21). They are the workflow's notes, not repo files.
