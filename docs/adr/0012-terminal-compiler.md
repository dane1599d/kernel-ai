# ADR 0012: The compiler in the terminal

Proposed, 2026-10-21. For the founder's approval; nothing here is decided until approved.

Repo citations are `path:line` on main at ec5ecff unless a branch is named. "Est." marks an estimate, not a measurement. "Unverified" marks what could not be read at the source. Design only: nothing was built, run or measured for this ADR. Facts were read on 2026-10-21 from the repo, the local cargo registry, crates.io, docs.rs and GitHub, on the founder's PC. No model ran anywhere.

## Context

The founder liked TempleOS's terminal, where every line is compiled and runs at once, and asked for that idea. He added that it is "not necessary if you can find a good open source one that fits the project". He did not ask for the HolyC language.

What is on main today:

- **The prompt is a Rhai interpreter, not a compiler.** Rhai 1.26.1, built `no_std`, `no_float`, `no_custom_syntax`, `no_module` (shellcore/Cargo.toml:12), runs in the ring-3 `shell` (kabi/src/plan.rs:9). Variables and functions persist across lines (shellcore/src/lib.rs:156-158, :280-288). A line gets 2 s, 8 MiB of heap and 6 KiB of output (shellcore/src/lib.rs:100-106) and 16 call levels (shellcore/src/engine.rs:100-109). One line per Enter, at most 200 characters (kernel/src/terminal.rs:21).
- **Model-written code is strict Rhai.** `while`, `loop`, `do`, `fn`, closures and `eval` are off (shellcore/src/script.rs:181-187). A script's tier is the strictest among the API names found in its text (shellcore/src/script.rs:197-211, shellcore/src/tiers.rs:55-60). Qwen3-4B scored 47/50 (94%) first try and after repair on the 50-task M8 test, on a GitHub runner in run 39675848260 (PR #10); this is recorded only in the PR body, not in docs/decisions.md.
- **Live code is built on the host.** Swappable code is Wasm components compiled on the host and run on wasmi 2.0.0 (decisions.md:15, :90; wasmhost/Cargo.toml:18). There is no compiler inside the OS.
- **W^X holds by construction.** No call makes a page executable or changes a page's protection (calls end at 36, kabi/src/call.rs:125). BRK and MODEL_MAP map only no-execute pages (kernel/src/process/loader.rs:181-184; kernel/src/model.rs:12-14), and a page gets NX unless it is asked for executable (kernel/src/memory/user.rs:134-139). Only ELF segments of boot modules are mapped executable (kernel/src/process/loader.rs:95-99; kernel/src/process/programs.rs:1-3), and uelf refuses writable-and-executable segments (uelf/src/lib.rs:154-157). NX itself rests on Limine: the kernel sets only EFER.SCE (kernel/src/cpu.rs:122-123) and never checks EFER.NXE.
- **Ctrl-C does not reach a running line.** The terminal waits in `ipc::call` with no cancel (kernel/src/terminal.rs:181). The per-process Ctrl-C flag (kernel/src/process/mod.rs:183-184) is cleared at spawn (:402) and set nowhere (grep at ec5ecff). The 2 s budget is what stops a line.

Rows that bind: decisions.md:14 (ring 3, no in-kernel linker), :15 (Rhai; plain Wasm on wasmi), :83 (a redefinition switches callers through a per-symbol slot; only HolyC's conveniences are imitated), :86 (fluency gate: 70% first try, 90% after one repair), :92 (the kernel never runs Wasm), :200 (own language and native JIT: deferred), :203 (autonomy by tier). docs/research/templeos-components.md:73 says "no source compatibility". The memo's plan was "a strict C subset that emits Wasm first" (docs/research/compiler-in-terminal.md:68).

Since the research ran: #15 (tiers, undo, show) and #17 (the Ember terminal; a prompt must end in `> `, termcore/src/style.rs:6, :112-119) merged. #16 claims call 37. The founder's 2026-10-21 rule: MIT, Apache-2.0, BSD, ISC and zlib crates need no approval if each PR lists them; GPL, LGPL and MPL-2.0 ask; C code in the build and non-crate tools stay founder questions. [ADR 0006](0006-existing-software.md) runs browsers in a Linux micro-VM, so the OS needs no JIT before v1.0 (decision sheet §3.2).

## Options considered

Three designs were judged. Scores are the judge's, out of 10.

| Design | What | For | Against | Result |
|---|---|---|---|---|
| A. Adopt holyc-lang | Vendor 36,780 of holyc-lang's 71,689 lines of C (BSD-2-Clause) as a ring-3 x86-64 JIT `hc`, with a new `cap::CODE` and two calls that map pages writable, then seal them executable | Closest to TempleOS: native code at the prompt, `Uf` disassembly | HolyC has little training data and no fluency figure. We would own a 37k-line fork, mostly one author's, whose README calls it "more for fun than for serious software development", plus a musl subset, eyalroz/printf, clang and the tree's first C. Most new kernel surface. Native code has no fuel. Conflicts with decisions.md:15, :83, :200, templeos-components.md:73 and ADR 0006 | 4, lost |
| B. C to Wasm with xcc | Run xcc's self-hosted `wcc` compiler as Wasm on wasmi in a ring-3 `cc` service; link each entry into a session; code runs on wasmi | A real compiler for a language with a large corpus. Code runs as soon as it compiles. The same compiler builds components in the OS. Every seam is a standard format. No kernel change | Rests on one unmeasured number, the compile latency of `cc.wasm` on wasmi. One-person upstream. We own a new linker. Its effects were queued until the run ended (fixed by A's need/resume graft) | 8, **won** |
| C. Rhai grows up | Rhai 1.27 with its Grain VM, plus a typed integer subset compiled by Cranelift 0.137 in a ring-3 `jit` | One language; keeps PR #10's 94% | The compiler covers only int and bool code, so most code gets at most Grain's gain (the 1.27.0 README claims "around 1.8-2x"). A second implementation of Rhai semantics over volatile internals. Cranelift `no_std` has only been type-checked in Wasmtime's CI, never linked or run. A JIT before v1.0, against ADR 0006. No builds in the OS | 5, lost |

Effort, as the designs estimated it (unmeasured): A 6-10 weeks part-time; B 5k-8k lines in 8 PRs; C about 5,700 lines plus two kernel calls.

Kept from the losers: A's need/resume effect loop, `:dis` (A's `Uf`), journal replay and the one-way seal (on paper only); C's Rhai 1.27 step, continuation prompt and NXE check.

Open-source reuse weighed (licences read at source on 2026-10-21):

| Project | Version | Licence | Verdict |
|---|---|---|---|
| xcc `wcc` (github.com/tyfkda/xcc) | main at 631e318 (2026-10-04); repo pushed 2026-10-21; no releases | MIT, "Copyright (c) 2024 tyfkda" | **Adopted**, unmodified, built into `cc.wasm` and the `kairt` runtime |
| wasmi | 2.0.0 (2026-09-01) | MIT/Apache-2.0 | In tree; runs the compiler and the sessions |
| wasmparser, wasm-encoder | 0.228.0, 0.262.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT (free) | **Adopted** for `kailink` |
| Rhai | 1.27.0 (2026-10-21T13:05Z) | MIT OR Apache-2.0; still requires smartstring 1.0.1 (MPL-2.0+) | Kept; upgrade behind a gate |
| holyc-lang | v0.0.15-beta (2026-07-28) | BSD-2-Clause | Reference only (design A) |
| Aiwnios | active, 2026-10-21 | BSD-3-Clause | Reference only |
| cranelift-codegen | 0.137.0 (2026-10-20) | Apache-2.0 WITH LLVM-exception (free) | The native tier, on paper only |
| Wasmtime `no_std` | 50.0.0 (2026-10-20) | Apache-2.0 WITH LLVM-exception | Runs precompiled code only; no compiler in `no_std` |
| QuickJS-ng | v0.17.0 (2026-09-18) | MIT | An option if a fluency test ever favours JavaScript |
| Roto, Pydantic Monty | 0.13.0, 1.2.0 (v1.3.0 tagged 2026-10-21) | BSD-3-Clause, MIT | Design references; both need `std` |
| TinyCC, CJIT, c4 | 0.9.27 (2017), -, - | LGPL-2.1, GPL-3.0, GPL-2.0 | Rejected: copyleft, and permissive options exist |
| chibicc, slimcc, cproc + QBE | -, -, QBE 1.3 | MIT, MIT, ISC + MIT | Rejected: no Wasm output; need an outside assembler |
| LuaJIT, WAMR, wasm3 | 2.1, 2.4.5, - | MIT, Apache-2.0 WITH LLVM-exception, MIT | Rejected: `std` and libc, or a second Wasm engine |
| Our own C subset on wasm-encoder | memo C4 | ours | Kept as the last fallback: 5k-10k lines, 2-4 months (compiler-in-terminal.md:240, :266) |

## Decision

The `kai>` prompt stays as it is, and a second mode, `c>`, lets you type C that compiles and runs the moment you press Enter, as TempleOS did. The compiler is xcc, an existing open-source C compiler (MIT), run as WebAssembly inside an ordinary ring-3 program, so the kernel does not change and no native code is made at run time. Anything a C program does to the system goes through the same tiers, log and undo as any other request. From P4 the same compiler also builds apps on the machine itself.

### D1. Languages

- **Rhai stays** the shell and the model's action language (decisions.md:15).
- **The prompt also compiles Kai C:** the C that xcc's `wcc` accepts, plus five conveniences done by our own front end, `promptcore`:
  1. a statement outside a function runs at once (wrapped in `void __kai_run_N(void)`);
  2. a trailing expression prints its value (a `_Generic` macro; xcc's lexer has `_Generic`, src/cc/frontend/lexer.c:63);
  3. a bare string prints itself;
  4. `Name;` calls a session function that takes no arguments;
  5. `kai.h` defines `U0`, `I8`-`I64`, `U8`-`U64`, `F64`, `Bool` and `Print`.
- **No HolyC source compatibility** (decisions.md:83 and templeos-components.md:73 hold). Not offered: `setjmp`, inline asm, compile-time execution, threads. wcc lowers `setjmp` to Wasm exception instructions (src/wcc/wcc_builtin.c:36, :98, :122) and wasmi 2.0.0 has no exception-handling switch (src/engine/config.rs:169-335).
- **Modes.** `:c` enters C mode and `:rhai` leaves it; `:c <entry>` runs one entry from `kai>`. The prompt is `c> `. In both modes, while braces or parentheses are open the shell replies `more` and the terminal shows `..> `, up to 4 KiB per entry. (The judges wrote `..`; the `> ` ending follows the Ember rule that a prompt ends in `> `.) In C mode, a line that does not compile and reads as plain words goes to the AI, as on `kai>` (decisions.md:204, shellcore/src/lib.rs:277).
- **`:` lines.** Today the terminal answers every `:` line itself and never sends it to the shell (kernel/src/terminal.rs:131-132, :295). It will pass `:c`, `:rhai` and, at `c>`, the session commands of D4 to the shell. None of these names is a kernel command today.

### D2. The pieces

| Piece | Kind | Job | Size (est.) |
|---|---|---|---|
| `third_party/xcc/` with `UPSTREAM.toml` | vendored C, pinned commit | ADR [0009](0009-licence.md)'s layout for whole trees | none of ours |
| `cc.wasm` | boot data | `wcc` compiled by itself (`make wcc-gen2`, Makefile:281-283), SHA-256 pinned; upstream's gen2-gen3 byte check (`wcc-diff-gen23`, :295-297) runs in CI; embedded in `cc` with `include_bytes!` | - |
| `kairt` | Wasm objects | the parts of xcc's libc we use (printf, string, malloc), built on the host with `wcc -c`, linked as entry 0 of every session | build rules |
| `wasihost` | no_std crate, host-tested | at most the 20 WASI preview-1 calls that xcc's libc declares (libsrc/_wasm/wasi.h), over an in-memory tree: `/kai` read-only (headers, `kai.h`, `kairt`), `/w` scratch. The clock returns 0, `random_get` is refused, `proc_exit` ends the compile | 600-1,000 |
| `kailink` | no_std crate, host-tested | links one relocatable object (tool-conventions Linking.md) into a session; reads with wasmparser 0.228.0 (already shipped by wasmi, Cargo.lock:1190); writes with wasm-encoder 0.262.0 `default-features = false` (today host-only, through `wat`, Cargo.lock:1133), whose wasmparser dependency is optional, so one parser ships | 1,200-2,000 |
| `promptcore` | no_std crate, host-tested | splits entries, applies the conveniences, writes the prelude, balances braces, keeps symbols, slots and the undo and source journals, renders `kai.h` | 800-1,200 |
| `cc` | ring-3 program and service | holds two wasmi Stores: the compiler's (a fresh instance per compile from one cached module) and the session's | 400-700 |

- **Plan line:** `cc caps=serve pages=24576 stack_pages=512 burst_ms=5000`. 96 MiB is a ceiling until spike S12-1; then it becomes the measured peak plus 25%.
- `cc` is built for the hard-float target, like `llm` (add it to `USER_FLOAT_PROGRAMS`, xtask/src/userbuild.rs:24), because the compiler's own code may use floats.
- **Total:** about 5k-8k lines of our Rust in 8 PRs (est.).

### D3. One entry, step by step

1. **Send.** The shell sends `cc` one CALL, at most 8,192 bytes (kabi/src/lib.rs:160).
2. **Prelude.** promptcore writes `/w/entry.c`: `#include <kai.h>`, the text of every live type and macro, a prototype for every live function, `extern` for every live global, then the entry.
3. **Compile.** A fresh `cc.wasm` instance runs `wcc -c -nostdlib -I/kai/include -o /w/entry.o /w/entry.c`. Fuel is spent in slices; between slices `cc` checks a 2 s deadline and Ctrl-C. wcc's errors are shown as they are.
4. **Link.** wcc's `-c` output is a standard object: a `linking` section plus `reloc.CODE` and `reloc.DATA` (src/wcc/emit_wasm.c:841, :941, :950).
   - kailink maps its memory, table and stack pointer to the session's.
   - It places the data at the next free address and gives each new function a table slot.
   - An undefined function becomes a thunk: `call_indirect` on that function's slot. A function pointer becomes the slot number.
   - It accepts only imports from module `kai` whose field is an action the registry has, plus WASI `fd_write` for fds 1 and 2 (output). Anything else fails to link, naming the symbol.
5. **Run.** wasmi validates and instantiates the module in the session Store, then calls `__kai_run_N` under fuel slices and the 2 s line budget.
6. **Commit.** On success, the new slots, the symbol table and both journals are updated. A compile error or a trap commits no definitions. Memory writes made before a trap stay, as in C.
7. **Reply.** Output (at most 6 KiB), the value, and one status line with compile, link and run times.

### D4. Sessions, redefinition and undo

- **A session is one wasmi Store:** memory of 1 MiB growing to at most 64 MiB, one funcref table of at most 4,096 slots (the same limit as wasmhost/src/instance.rs:143), and a shared stack pointer with a 64 KiB stack.
- **Redefinition.** A same-type redefinition writes the function's slot, so every caller and every stored function pointer switches (meets decisions.md:83). TempleOS does not do this: its existing callers keep a direct `CALL` (compiler-in-terminal.md:17). A different type gets a new slot; old callers keep the old body, and the reply names them. A global with the same name and type keeps its address.
- **`undo`** at `c>` reverts the last entry: the slots and symbols it replaced, and, through [ADR 0002](0002-agent-actions.md), its log entry. Memory writes are not undone. clang-repl's `%undo` is the precedent.
- **Commands** start with `:`: `:dis f` prints f's Wasm text (a small printer over wasmparser), `:defs` lists definitions, `:reset` drops the Store. `undo` is the same word as at `kai>` and is reserved.
- **Compaction.** wasmi keeps every instance until its Store drops. After 512 entries (est.), `cc` relinks every live definition from its kept source into one module at the same addresses and slots. This works because kailink, not the compiler, picks addresses.
- **Restart.** The shell keeps the source of every defining entry. If `cc` dies, the supervisor restarts it. The shell replays definitions, never statements, and says so: `cc restarted (trap in "Foo(3);"); restored 12 definitions; globals reset`.
- **From P2:**
  - `:save <name>` writes the live definitions to `/home/user/c/<name>.c`, a plain C file that starts `#include <kai.h>`; `:load <name>` replays it.
  - `#include "<path>"`: the shell reads the file through [ADR 0004](0004-storage.md) under the request's grant and passes it to `cc`, in an ADR [0001](0001-capabilities.md) `Shm` when over 8 KiB. `cc` never holds a file handle.

### D5. Security model

- **No executable memory is created at run time before v1.0.** Compiled code is data that wasmi interprets. There is no new system call and no capability bit: `cc` holds handles, no cap bit (sheet §3.8). Today that is `serve`; under ADR 0001, one service endpoint and no routes.
- **Three walls.** The compiler's Store sees only the in-memory tree. Session code reaches only its imports. The `cc` process holds no authority, so a wasmi bug stays inside it and the supervisor restarts it. wasmi stays pinned and its advisories are followed (CVE-2025-66627, a use-after-free, CVSS 8.4, fixed in 1.0.1).
- **Imports are the exact capability list.** Wasm code cannot reach a host function it does not import. This is sound, unlike `names_in`'s text scan, which can only list too much.
- **Effects: need and resume.**
  - A `kai` import returns a host error, which wasmi turns into a resumable call (`ResumableCallHostTrap`, wasmi-2.0.0 src/engine/executor/mod.rs:95).
  - `cc` replies `need <seq> <call>`. The shell passes the call to ADR 0002's one door, `agentcore::execute(call, consent)`, under the request's grant node (ADR 0001), and sends back `resume <seq> ok|err <result>`.
  - So a program sees its own results, and every effect gets 0002's tier, consent, log and undo.
- **Transactions.** One entry, or one model program run, is one ADR 0002 transaction and one `actlog 1` entry. A trap or error rolls back its OS changes. Outbound effects flush only on success.
- **Data labels.** A C value cannot carry ADR 0002's `Data` label. So `cc` labels the run instead: after any call that returns Content, every later undo-less or outbound call in that run counts as Data-controlled and asks.
- **Fuel and Ctrl-C.** wasmi 2.0.0 exports `ResumableCallOutOfFuel` (src/lib.rs:193), so a run can be resumed slice by slice (S12-1 confirms). Between slices `cc` checks the deadline and the INTERRUPTED call (kernel/src/process/syscall.rs:142). Ctrl-C delivery, which main lacks, is also needed by ADR 0002's rollback; whichever PR comes first adds it: the terminal reads keys during a call and sets the flag of the process running the request.
- **Model-written C** runs in a fresh session per request, after a trial run on stub imports. `:keep` copies its definitions into the user's session after showing them.
- **NX.** A boot check that EFER.NXE is set on every CPU (D9), because W^X rests on it.

### D6. The `kai` imports and `kai.h`

- One import per ADR 0002 registry row: module `kai`, field = the action name (snake_case, no dots, at most 24 bytes, a C identifier; sheet §2). A name never changes meaning; imports are versioned with `REGISTRY_VERSION`.
- Lowering: `Int` is `I64`. `Enum`, `Text`, `Path` and `Host` are UTF-8 `(pointer, length)` pairs, written `const char *` in C. 0002's `List` and `Opt` are lowered in the same PR. The result is an `I64` status, 0 or a negative `KAI_E*` code. Text results are copied into a buffer the caller passes; records fill a generated struct through an out pointer. A golden test fixes the lowering.
- `kai.h` is rendered by promptcore at session start from the live registry: OS rows and installed apps' `kai-actions 1` rows, never-tier rows left out. Above 8 KiB it travels in an `Shm`.

### D7. Components built in the OS

- `cc build <component>` compiles C sources in `/w` (from P2, the component's directory), links standalone (its own memory, imports by [ADR 0003](0003-app-model.md)'s world), and writes 0003's artifact unchanged: `kai.manifest` (`kai-manifest 2`, `engine wasmi`), `kai.source`, `kai.actions`, `kai.app` and `kai.notices`, which carries xcc's MIT notice for `kairt`.
- **v0.1 scope:** components on the in-tree world `kai:app/component`, and apps that declare no actions of their own. `kai:app/app` alone exports only `events`, and ADR 0003 §5 requires every app world to export `actions` (it may be empty) and has the build service refuse a world without it. So an app built in the OS exports an empty `actions` interface. Both C bindings, for `kai:app/component` and for a world that includes `kai:app/app` and exports an empty `actions`, are generated on the host by wit-bindgen 0.63.0's C generator and shipped read-only in `/kai`. `cc build` writes a `kai.actions` with its header and world line and no action rows. An app with its own `app:<name>` actions needs bindings and `kai.actions` from the host build service; generating them in the OS needs WIT tools built for our target (unverified) and is not in v0.1.
- **Provenance.** The ledger records `built-by cc <xcc commit> source <sha256>` where decisions.md:98 requires a 40-hex git commit (founder question 4). The ledger-to-git outbox (decisions.md:103) commits the build once a host is connected. Installing it as an app uses [ADR 0011](0011-packages.md)'s local repository `local`, signed by an Ed25519 device key.
- Rust components keep the host build service. Both kinds run on the same `wasmhost`.

### D8. The AI

- The model writes Kai C only when asked for a program or component, and only after spike S12-2 passes. Plain requests stay on 0002's tagged calls and `script(code)` in Rhai.
- Route: 0002's `script` pseudo-action gains `lang` (`rhai` by default; `c` enters the grammar only after S12-2). Same tiers, transaction and log. ADR 0002 owns that change.
- Compiler errors are the reason given in the one repair round (decisions.md:75). Type errors are caught before anything runs.
- The prompt card: a Kai C card rendered by ADR [0010](0010-skill-pack.md)'s `llmproto::pack::render` from the same registry, as a new `Format` (`c-1`), used only for program requests. S12-2 measures its token cost against 0010's budget.

### D9. Grafts on Rhai and the terminal

- **Rhai `=1.27.0`** in shellcore/Cargo.toml:12 and spikes/rhai/Cargo.toml:12, at the start of P1 so ADR 0002's analyser is written once against it. A redefinition test (none exists). Grain becomes the default only if S12-3 passes.
- **Rhai's string type** (ADR 0009 hands this to this ADR). 1.27.0 still requires smartstring, which RUSTSEC-2026-0249 (dated 2026-05-03, published 2026-08-10) marks unmaintained; `compact_str` (0.10.0, MIT) is optional only. We offer Rhai upstream a small PR that makes smartstring optional when `compact_str` is on. If no release has it by 0009's review date, 2027-01-31, we patch Rhai with that one change (`[patch.crates-io]`, `third_party/rhai/UPSTREAM.toml`). Whether `compact_str` builds for our target is unverified; that PR checks it. Until then smartstring ships under 0009's question 3.
- **`more` replies** from the shell, so the terminal draws `..> ` in both modes.
- **The EFER.NXE boot check.**

### D10. The native tier, recorded on paper only

Ring-3 Cranelift in a single-thread process, with two one-way calls: map pages writable and not executable, then seal them executable and never writable again. A single-thread holder needs only a local `invlpg` (kernel/src/memory/user.rs:5-12). decisions.md:200 stays "Deferred" for native code. It is reconsidered no earlier than v1.0, only if a P4 profile shows wasmi dispatch taking at least 50% of a workload the user waits on, and only with the founder's approval. Its input would always be wasmi-validated Wasm, model sessions would stay on wasmi, and its calls would be named in an ADR and numbered when they land (sheet §2).

### D11. Wire format `kai-cc 1`

| Shell to `cc` | Meaning | `cc` to shell | Meaning |
|---|---|---|---|
| `open <session> origin=user\|model` | start a session (with `kai.h`) | `need <seq> <call>` | an effect, as ADR 0002's log text |
| `eval <seq>` + source | compile, link and run | `out <text>` | printed output |
| `resume <seq> ok\|err` + result | an effect's answer | `def <kind> <name> <slot>` | a definition this entry added |
| `undo`, `dis <name>`, `defs`, `reset` | session commands | `done <type> <value> <compile_us> <link_us> <run_us>` | the end |
| `build <component>` | build in the OS | `err <line>:<col> <text>`, `trap <reason> <function>` | a failure |

## Consequences

- TempleOS's idea, with isolation TempleOS lacked: typed code at the prompt that runs as soon as it compiles.
- A language with a large training corpus, and builds on the machine itself, with no kernel change.
- Every effect a program makes goes through the same tiers, log and undo as any request.
- Two languages share the prompt: Rhai for actions and quick commands, C for typed code. The user and the model must know which to use.
- C is memory-unsafe inside a session: a stray pointer can corrupt another definition's data, never the OS.
- Code is interpreted. It may be about 10x slower than native (anecdotal, compiler-in-terminal.md:151), from a compiler with no optimiser.
- We depend on a one-person compiler and own a new linker of about 2k lines. Faults name a function, not a line, until wcc gains line tables (unverified that it has none).
- Every entry re-parses the prelude, so cost grows with the session until compaction or a prelude cache.
- The build needs a host C compiler, and maybe node.js, npm and `llvm-ar` (xcc README).

## What it costs to change later

| Choice | Cost |
|---|---|
| The compiler (xcc) | Low: the seam is a standard Linking.md object; clang or our own front end can replace it |
| The engine (wasmi) | Low: modules are core Wasm |
| Session layout and journals | Low: private to `cc`; saved sessions are plain C files |
| Adding the native tier | Medium: two new kernel calls and changes inside `cc` only |
| `kai` import names, lowering and `kai.h` | Medium once people write code against them; versioned with the registry |
| The in-OS provenance rule | Medium once ADR [0008](0008-updates.md) and 0011 sign artifacts |
| C as the typed language | High: user programs and the model's card depend on it. HolyC syntax later would break saved sessions, though not the linker or runtime |

## Spikes that settle it

Spike ids carry this ADR's number, so they do not collide with other ADRs' spikes.

**S12-1 `cc-wasm`.** Branch `spike/cc-wasm`, never merged. GitHub Actions only, under KVM and TCG with `-cpu max`, as boot case `test=ccspike`, plus host tests. No model runs and nothing runs on the founder's PC.

- **Build:** xtask builds `cc.wasm` from a pinned xcc commit and records which host tools it needed (gcc, `llvm-ar`, node); a minimal `wasihost` and `kailink`; a corpus of 20 entries from 1 to 150 lines, each with preludes of 0, 200 and 1,000 lines; the `notes` component's logic ported to C; a 1,000,000 sieve and a 10-million-step sum, in Kai C and in Rhai 1.26.1 AST and 1.27.0 Grain at `kai>`.
- **Measure:**
  - (a) `cc.wasm` validates under wasmhost's proposal set (wasmhost/src/lib.rs:50-71), with no exception, tail-call or SIMD instructions;
  - (b) translate time and memory of `cc.wasm`;
  - (c) per entry: compile, link, instantiate and run times, p50 and p95 over 5 boots;
  - (d) the compiler's peak linear memory and fuel per compile;
  - (e) the speed ratio against Rhai;
  - (f) the need/resume round trip, shell to `cc` and back;
  - (g) a redefinition switching an existing caller and a stored function pointer;
  - (h) report only: whether wcc compiles wit-bindgen 0.63.0's C output for `kai:app/component` and for D7's app world with an empty `actions`.
  - Run (b)-(e) under both wasmi dispatch modes ADR 0003's spike compares, and use the one 0003 picks. `portable-dispatch` alone is today's (decisions.md:93).
- **Pass (all, under KVM):** (a) holds; 200-line prelude p50 at most 150 ms and p95 at most 400 ms; 1,000-line prelude p50 at most 400 ms; compiler memory at most 32 MiB; Kai C at least 10x faster than Rhai AST on both loops; need/resume median at most 100 µs; (g) passes. TCG figures are reported only.
- **Fail:** p50 above 500 ms, memory above 64 MiB, or (a) false. Fallbacks in order, each keeping the rest of the design: a warm compiler instance restored from a memory snapshot; a prelude cache; our own C-subset front end on wasm-encoder. Between the pass and fail bars, the founder decides.

**S12-2 C fluency.** `fluency-model.yml` on a GitHub runner (decisions.md:185), never the founder's PC. The 50 M8 tasks are ported to Kai C against a generated `kai.h`; each answer is compiled by `cc.wasm` on the host and run against the stand-in OS. Bar: 70% first try and 90% after one repair before the model may write C programs. Replacing Rhai as the action language would need C to beat Rhai's 47/50; that is not proposed.

**S12-3 Grain gate.** Host tests on a GitHub runner plus a KVM boot case in CI. Pass: 0 mismatches between AST and Grain over the 50 reference scripts and the fuzz corpus, and at least 1.2x on the shell's micro-benchmarks. Otherwise AST stays the default.

## Phases that depend on it

| Phase | What it needs |
|---|---|
| P1 | Rhai 1.27.0 and S12-3; `more` replies; the NXE check; S12-1; `agentabi` rows (0002) from which `kai.h` and the bindings are generated; `agentcore::execute` as the one door; Ctrl-C delivery (also 0002's); `wasihost` and `kailink` begin as host-tested crates |
| P2 | `promptcore`; files for `#include`, `:save` and `:load`; source journals on disk (0004) |
| P3 | Multi-line input in the terminal app on ADR [0005](0005-graphics.md)'s surfaces |
| P4 (v0.1) | The `c>` prompt, sessions, `:dis`, `undo`, `cc build` beside the terminal and editor, `kai.notices` in built artifacts, installs from `local` (0011); model-written C only if S12-2 passes |
| P5 | Nothing new: network actions reach C as `kai` imports at 0002's tiers |
| P6 | None: JITs, rustc and clang run only inside the Linux guest (0006) |
| P7 | S12-1's numbers re-measured on the founder's PC (no model involved) |
| P8 | Fuzzing `kailink`, `wasihost` and `promptcore`; a security review of the `cc` boundary; wasmi and xcc advisories tracked; the native-tier question at v1.0 |

## Changes to recently merged and open work

| Work | Change | Files | Why |
|---|---|---|---|
| #15 seamless actions (merged) | None now. In P1, 0002's registry PR moves each action body out of the Rhai closures into `agentcore::execute`, with the undo record made there | shellcore/src/script.rs:61-150 (`register`), shellcore/src/undo.rs:20-27 | Rhai, `scriptrun` and `cc`'s need/resume must share one door, and `undo` must cover C effects. `TIERS` (shellcore/src/tiers.rs:35-44) stays data until generated |
| #17 Ember terminal (merged) | None now. With the `more` reply (P1) and `c>` (P4): `prompt_width` learns the `c` and `..` heads, with tests beside the existing ones; the prompt comes from the mode the shell reports, as `edit_mode::prompt()` does; the reply wire gains `more`; `:c`, `:rhai` and the session commands go to the shell (D1) | termcore/src/style.rs:112-119, :170-175; kernel/src/terminal.rs:101-105, :131-132, :192-201, :295; shellcore/src/lib.rs:144-151 | Without it the new prompts are not coloured as prompts. `MAX_LINE` stays 200 (kernel/src/terminal.rs:21) |
| #13 parser hardening (merged) | When they land, `kailink`, `wasihost` and `promptcore` each get `tests/hostile.rs` including testkit/hostile.rs, and its header list gains them | testkit/hostile.rs:1-4 | They parse untrusted objects and source |
| #14 AVX2 attention (merged) | None | - | Unrelated |
| #16 DHCP and DNS (open, RESOLVE = 37) | None | - | This ADR adds no system call |
| #18 virtio-gpu, #19 USB input (open) | None | - | Neither touches the shell, the prompt or memory protection |
| PR #10 (merged) | Record 47/50 (94%), run 39675848260, as a row after decisions.md:185 | docs/decisions.md | S12-2's baseline is in a PR body only |
| Test builds | `spikes: false` by default; keep `--spikes`; update the test | xtask/src/options.rs:112, :136, :306-309 | `cargo xtask test` (ci.yml:79) links Rhai and wasmi into ring 0 (kernel/Cargo.toml:14-15), against "No JIT or compiler in ring 0" (compiler-in-terminal.md:58) |
| Boot checks | Fail boot if EFER.NXE is clear; the equality check then covers every CPU | kernel/src/cpu.rs:122-123, :163-178 | W^X rests on NX, which only Limine sets today |
| Decision rows, on approval | decisions.md:15 gains the C mode; :83 records how the slot rule is met; :200 becomes "C through xcc to Wasm adopted; native tier deferred past v1.0"; :98 gains the provenance line if question 4 is yes; roadmap.md:143 adds the `c>` prompt and in-OS builds to P4 | docs/decisions.md, docs/roadmap.md | Keep the record true |

## Open questions for the founder

1. **The prompt language.** Is "C with HolyC's conveniences and type names" what you meant by TempleOS's terminal compiler? Or do you want real HolyC syntax (design A), at the cost of a 37k-line C fork and a language the model barely knows?
2. **Model-written programs.** Once S12-2 passes, may the model compile and run its own C at the run tier when you ask for a program? Or should a model-written program always be shown first?
3. **Native code.** Do you accept "no executable memory created at run time before v1.0", as ADR 0006 also proposes? It trades TempleOS's native speed (perhaps 10x, unmeasured) for a smaller attack surface.
4. **Builds in the OS.** May components built on the machine be applied without a git commit, under the source-hash provenance line? This relaxes decisions.md:98 and touches 0008's and 0011's signing.
5. **xcc and its tools.** If your answer to ADR 0009's question 4 covers C compiled into the image and host-only tools, nothing more is needed here. If not: do you approve xcc (MIT), compiled to Wasm, in the image; and gcc or clang, and possibly node.js, npm and `llvm-ar`, in CI and in WSL2 on your PC?

New crates in the shipped graph need no question under your 2026-10-21 rule and will be listed in their PRs: wasm-encoder 0.262.0 (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT) and leb128fmt 0.1.0 (MIT OR Apache-2.0), wasmparser 0.228.0 (same as wasm-encoder) as a direct dependency, rhai 1.27.0 and rhai_codegen 3.3.0 (MIT OR Apache-2.0), and compact_str 0.10.0 (MIT) if D9's patch is used. smartstring stays under ADR 0009's question 3.

## Sources

Repo (main at ec5ecff): docs/decisions.md:14, :15, :52, :75, :83, :86, :90, :92, :93, :98, :103, :185, :200, :203, :204; docs/roadmap.md:131, :139-147; docs/research/compiler-in-terminal.md:17, :58, :68, :151, :240, :266; docs/research/templeos-components.md:73; shellcore/Cargo.toml:12; shellcore/src/lib.rs:100-106, :144-151, :156-158, :277-288; shellcore/src/engine.rs:100-109; shellcore/src/script.rs:61-150, :173-211; shellcore/src/tiers.rs:35-60; shellcore/src/undo.rs:20-27; spikes/rhai/Cargo.toml:12; kabi/src/plan.rs:9; kabi/src/lib.rs:82-106, :160, :234-249; kabi/src/call.rs:125; kernel/src/terminal.rs:21, :101-105, :131-132, :181, :192-201, :295; kernel/src/cpu.rs:122-123, :163-178; kernel/src/memory/user.rs:5-12, :134-139; kernel/src/process/mod.rs:183-184, :402; kernel/src/process/syscall.rs:142; kernel/src/process/programs.rs:1-3; kernel/src/process/loader.rs:95-99, :181-184; kernel/src/model.rs:12-14; kernel/Cargo.toml:14-15; uelf/src/lib.rs:154-157; wasmhost/Cargo.toml:18; wasmhost/src/lib.rs:50-71; wasmhost/src/instance.rs:143; termcore/src/style.rs:6, :112-119; testkit/hostile.rs:1-4; xtask/src/options.rs:112; xtask/src/userbuild.rs:24; .github/workflows/ci.yml:79; Cargo.lock:696, :856, :1133, :1153, :1190, :1199. Branch origin/dane/net-dhcp-dns-b5bdb7: kabi/src/call.rs:134 (RESOLVE = 37). Work files: D:/kai-targets/p0/work/12/{judgement,design-A,design-B,design-C,research-code,research-prior}.md and D:/kai-targets/p0/work/decision-sheet.md.

External (read 2026-10-21):

- xcc: https://github.com/tyfkda/xcc (main at 631e318; LICENSE MIT; README "WebAssembly" requirements; Makefile:281-297; src/wcc/emit_wasm.c:841, :941, :950; src/wcc/wcc_builtin.c:36, :98, :122; src/cc/frontend/lexer.c:63; libsrc/_wasm/wasi.h)
- WebAssembly tool-conventions, Linking.md (main): https://github.com/WebAssembly/tool-conventions/blob/main/Linking.md
- wasmi 2.0.0 (2026-09-01), local registry source src/lib.rs:193, src/engine/executor/mod.rs:95, src/engine/config.rs:169-335; release post: https://wasmi-labs.github.io/blog/posts/wasmi-v2.0/
- wasmi CVE-2025-66627: https://github.com/wasmi-labs/wasmi/security/advisories/GHSA-g4v2-cjqp-rfmq
- wasm-encoder 0.262.0 (2026-10-13) and wasmparser 0.228.0 (2025-04-01): https://crates.io/api/v1/crates/wasm-encoder/0.262.0, https://crates.io/api/v1/crates/wasmparser/0.228.0
- rhai 1.27.0 (2026-10-21T13:05Z) dependencies: https://crates.io/api/v1/crates/rhai/1.27.0/dependencies; Cargo.toml: https://docs.rs/crate/rhai/1.27.0/source/Cargo.toml.orig
- RUSTSEC-2026-0249, smartstring unmaintained (dated 2026-05-03, added to the database 2026-08-10): https://rustsec.org/advisories/RUSTSEC-2026-0249.html
- cranelift-codegen 0.137.0 (2026-10-20): https://crates.io/crates/cranelift-codegen/0.137.0
- GHSA-897f-g5gm-fmxm (Cranelift, Critical, CVSS 9.9, 2026-10-19; fixed in Wasmtime 36.0.19 and 43.0.0): https://github.com/bytecodealliance/wasmtime/security/advisories/GHSA-897f-g5gm-fmxm
- Wasmtime `no_std` is AOT only: https://docs.wasmtime.dev/stability-platform-support.html
- holyc-lang (v0.0.15-beta, 2026-07-28; COPYING BSD-2-Clause): https://github.com/Jamesbarford/holyc-lang
- Aiwnios: https://github.com/aiwnios/Aiwnios; QuickJS-ng v0.17.0: https://github.com/quickjs-ng/quickjs; Roto 0.13.0: https://github.com/NLnetLabs/roto; Monty 1.2.0 on crates.io (v1.3.0 tagged on GitHub 2026-10-21T17:56Z): https://github.com/pydantic/monty
- TinyCC: https://bellard.org/tcc/; c4 LICENSE: https://raw.githubusercontent.com/rswier/c4/master/LICENSE
- clang-repl `%undo`: https://clang.llvm.org/docs/ClangRepl.html
