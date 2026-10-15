# Compiler in the terminal: Rhai REPL, JIT options and LLM-written scripts

**Date:** 2026-10-14

Merged from two independent research passes (local and cloud).

- **Project:** Kernel AI
- **Scope:** how TempleOS compiles shell input into memory, which in-OS language and execution engine fit a `no_std` ring-3 host, and how LLM-written scripts are constrained, shown, confirmed and run. Kernel feasibility is in [rust-kernel-foundation.md](./rust-kernel-foundation.md). Milestones M0-M10 are defined there.
- **Related memos:** [live-editable-system.md](./live-editable-system.md), [llm-in-a-from-scratch-os.md](./llm-in-a-from-scratch-os.md), [templeos-components.md](./templeos-components.md)

## Conclusions

1. **Copy TempleOS's experience, not its mechanism.** Its terminal compiles each statement to x86_64 in heap memory and CALLs it (no interpreter, no byte code). That works only because everything runs in ring 0 in one identity-mapped address map. An embedded interpreter in ring 3 gives the same feel: expressions at the prompt, runtime include, persistent symbols, one language for shell and programs, built-in disassembly and trace tools.

2. **ZealOS is the same compiler under new names.** Use it and holyc-lang as readable references only. holyc-lang (2026) is the useful counterpoint: it runs a HolyC JIT in user space under W^X (map read-write, then switch to read-execute).

3. **TempleOS does not update existing callers when a function is redefined.** A call to an already-defined function is a direct `CALL` to the old address. Only calls compiled while the function was still `extern` go through its `exe_addr` slot. Live editing therefore needs a per-symbol slot or patchable stub, one indirect call per use. Both passes and [templeos-components.md](./templeos-components.md) agree.

4. **The REPL language is Rhai (founder decision).** Rhai 1.26.1 (2026-09-10, MIT OR Apache-2.0) is `no_std` with `alloc`, actively released, sandboxed by default, has operation and size limits, and keeps REPL state in a `Scope`. Costs both passes found:
   - No formal grammar, so any GBNF is a hand-written approximation.
   - Niche language, so models know it less than Lua, Python or JavaScript. Rhai ships a cheat sheet written for AI agents.
   - The released crate depends on `smartstring` (MPL-2.0+, unmaintained, RUSTSEC-2026-0249). A `compact_str` (MIT) replacement exists only in unreleased 1.27.0. This matters to any permissive-only licence policy.
   - A running script can be aborted but not suspended, so it needs a preemptible process.
   - Disputed: does Rhai build on stable? Local: the library builds for bare metal on stable, because Rhai's CI runs `cargo check --lib` for `riscv32imac-unknown-none-elf` with `no_std` on stable. Cloud: the Rhai book says `no_std` "requires the nightly compiler". Resolution: both are true. CI only type-checks one riscv target, and the book's nightly note concerns the demo binary. We pin a nightly anyway, so the dispute does not change the plan. Build and run it on `x86_64-unknown-none` early to confirm.

5. **Interpreter first, no JIT in the POC.** A shell is latency and I/O bound. Rhai runs about 1 million loop iterations in 0.14 s (AST walker, 2.6 GHz single core). LLM generation of a 300-token script takes 7.5-20 s, which dwarfs interpretation.
   - Disputed: Grain bytecode VM speedup. Local: about 1.8-2.2x faster than the AST interpreter since 1.26.0. Cloud: 1.8-2x on `switch` statements only. Resolution: cloud. The Rhai changelog (checked 2026-10-14) gives 1.8-2x for `switch`, 2x for `!<bool>`, "at par or faster" for index assignments and callbacks, in unreleased 1.27.0, and no figure at all for 1.26.0. Keep the AST engine; move to Grain after 1.27.0 ships and is re-tested.

6. **Never compile or JIT untrusted code in ring 0.** The REPL, the compiler, the interpreter and all generated code run in ring 3 with a separate address space per script, W^X, SMEP, SMAP, UMIP and capability handles (founder decision for shell scripts). The local pass's "ring 0 for milestone 1, read-only capabilities" option is superseded. Native code from a prompt, if ever added, runs in ring 3 and asks the kernel to map pages read-execute.

7. **Compiled, hot-swappable components are plain WebAssembly modules on `wasmi` (founder decision).** wasmi 2.0.0 (2026-09-01) is a `no_std` interpreter with fuel metering and resumable out-of-fuel calls, audited twice, and it is the only isolation layer of the Rust hobby OS Munal OS. It cannot parse WAT without `std`, so modules are built on the host. Wasmtime in `no_std` is runtime-only (no Cranelift or Winch), so modules must be precompiled elsewhere by the identical version. Keep it as a later performance tier.

8. **Other languages fall short for the POC.** A custom HolyC-like language costs months and has no training data (TempleOS's compiler is 18,634 lines; holyc-lang's JIT commit alone was 30,522). No pure-Rust Lua builds for `no_std`; C Lua needs a libc subset and a `longjmp` across Rust frames is undefined behaviour. Stak Scheme has a tiny `no_std` VM but a `std` compiler wrapper. Rune is `no_std` but a 2.8 MB crate. Koto, Gluon, Dyon, Mun, Boa and mlua need `std`. Forth is a debug-monitor language at best. A new language stays post-POC.

9. **A native JIT in `no_std` is possible but has no ready path.** `cranelift-codegen` and `cranelift-frontend` are `no_std` and cargo-checked for `x86_64-unknown-none` in Wasmtime's CI (type-checked, not run). `cranelift-jit` and `cranelift-native` need `std`, so we would write our own code-page allocator, relocation patcher and W^X switch. `iced-x86` (`no_std`, `code_asm`) is the lightest encoder for a template JIT. `dynasmrt` is `std`-only. Copy-and-patch is the cheapest documented route. Any JIT runs in ring 3.

10. **Running `rustc` in-OS is not realistic.** Use a host build service for Rust components. Redox, with a full libc, reported rustc and cargo running natively only in January 2026.

11. **Small models write unfamiliar syntax badly, so the target language must be C-family, small and taught in the prompt.** Language skill tracks training share. A syntax reference of about 2,100 tokens was the best prompt-side fix on Cangjie. Few-shot examples help 1B-7B models only slightly. Grammar in the prompt and at decode time both help. The biggest POC risk is model fluency, not the engine.
    - Disputed: how to test it and what to do on failure. Local: 100 intents in week 1, bar at least 90% success after at most two repairs, fallback MicroPython's embed port in ring 3. Cloud: 50 synthetic tasks, with first-pass run success under about 70% arguing for building our own language sooner. Resolution: run 50 tasks (extend to 100 if results are close), report both first-pass and after-two-repairs success, require at least 70% first pass and 90% after repair. Run it on the dev host against the Mac mini now, because it needs no OS code. If it fails after prompt, cheat-sheet and grammar work, fall back to MicroPython (weeks of work, C plus a libc shim, ring 3) before writing our own language (months).

12. **Fixed LLM sequence: parse first, LLM second.** Input that parses and resolves runs at once. Otherwise: free explanation, then a grammar-constrained code block (grammar built per request, naming only the allowed API), local re-parse with the OS's own front end, capability list derived from the parse, show, confirm, run in a fresh process holding only the granted handles, append-only log, at most two repair rounds (self-repair gains are modest). Reject answers that stopped on the token limit.

13. **Of three candidate servers, only llama-server accepts a raw grammar.** Send script generation there, or use a JSON-schema envelope plus validate-and-retry on any backend. `mlx_lm.server` has no constraint support; Ollama and LM Studio (MLX engine) take JSON Schema only. Grammar sampling roughly doubled per-token time in JSONSchemaBench on server hardware, so measure tok/s with a grammar on the M4.

14. **The grammar is a syntax filter, not a safety control, and confirmation is not a security boundary.** Constrained decoding removed 96% of syntax errors but cannot enforce scope, types, termination or completeness. Users approve 93% of agent permission prompts, and prompt injection has no known fool-proof prevention. The kernel enforces every capability; the derived list only shows intent, and it can be incomplete if a script calls through a value.
    - Disputed: approval policy. Local: auto-run scripts whose computed capability set is read-only, confirm anything that mutates state. Cloud: confirm everything once, default answer No, no "always allow" for generated code. Resolution: cloud, for the POC. It cites OWASP and approval-fatigue data and the derived capability list is not enforcement. Revisit a read-only auto-run tier after indirect calls and `eval` are shown to be switchable off in the LLM engine configuration.

15. **No analysis guarantees a script terminates.** Termination comes from budgets (Rhai `set_max_operations`, `on_progress`, optional loops-off; wasmi fuel) with the kernel's preemptive timer and kill as the backstop.

16. **Test the sandbox under KVM and under TCG with `-cpu max`.** TCG emulates NX, SYSCALL, SMEP, SMAP, UMIP and PKU. The default `qemu64` model exposes no leaf-7 features, so SMEP stays off without `-cpu max`. PCID is missing under TCG, so treat it as optional.

## Decisions

| Decision | Status | Default or choice |
|---|---|---|
| REPL language | Decided (founder) | Rhai |
| Where shell scripts and generated code run | Decided (founder) | Ring 3 under a restarting supervisor, including code the founder types. No JIT or compiler in ring 0 |
| Format for compiled, swappable components | Decided (founder) | Plain WebAssembly modules on wasmi |
| LLM placement | Decided (founder) | Remote only, control plane only |
| Execution engine | Open | Rhai AST interpreter; switch to Grain after 1.27.0 ships and is re-tested |
| Approval policy for LLM scripts | Open | Confirm every script; no always-allow |
| Server for script generation | Open | llama-server with a per-request GBNF grammar; JSON-schema envelope as fallback |
| Loops in scripts | Open | Two engine configurations: loops off plus operation budget for LLM scripts, loops on plus budget for founder-typed code |
| Redefinition semantics | Open | Per-symbol indirection slot, so existing callers switch |
| FPU policy for ring-3 code | Open | Kernel stays soft-float. User target soft-float and Rhai `no_float` in the POC. Enable SSE with XSAVE only when a hard-float user target is needed |
| Fluency gate and fallback | Open | 50-task eval; fallback MicroPython embed port in ring 3, then own language |
| Own language | Open | Defer past M10; if built, a strict C subset that emits Wasm first, then a Cranelift backend in ring 3 |
| Native JIT | Open | Post-POC, ring 3 only, W^X |
| Compile-time execution (`#exe`-like) | Open | Allowed only in founder-typed code, never in LLM output |
| Capability taxonomy | Open | Start coarse (console, read-only metrics, policy.propose, network, file by path); refine after approval-fatigue data |
| Permissive-only dependency policy | Open | Permissive-only in the kernel and ring-3 core; decide on MPL (`smartstring`, `dynasmrt`) before M8 |

Superseded by founder decision: ring-0 REPL for the first milestone, floats in ring 0 (soft-float acceptance), Wasm hosted in the kernel, a language that compiles to Wasm on-box as the live-edit unit.

## Evidence

### 1. TempleOS: how the shell compiles into memory

Files are from the `archive` branch of https://github.com/cia-foundation/TempleOS (last commit 2020-02-24, c26482b, https://github.com/cia-foundation/TempleOS/commit/c26482bb6ad3f80106d28504ec5db3c6a360732c; last authored snapshot 2017-11-21). Base URL `https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/`.

- Command line: "The cmd line feeds into the HolyC compiler line-by-line as you type. A stmt outside a function executes immediately." Zero-argument calls need no parentheses; a bare string prints; there is no `main()`. https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/CmdLineOverview.DD
- Davis: the shell "places code into memory it MAlloc()s", "nothing is interpreted and there is no byte code", boot "takes only two seconds" (his claim, no measurement). https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/Welcome.DD
- `ExeCmdLine` calls `LexStmt2Bin` per statement and runs `Call(machine_code)`, records the answer and elapsed time, frees the code unless `CCF_HAS_MISC_DATA` is set. https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Kernel/KTask.HC
- `LexStmt2Bin` parses one statement to reverse-Polish intermediate code, appends a return and calls `COCCompile`; `ExePutS`, `ExePrint`, `ExeFile` (builds `#include "file";`). https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Compiler/CMain.HC
- `COCCompile` runs `OptPass012` twice, passes 3-6, then `OptPass789A` until size stops shrinking; JIT output is `MAlloc(code_size, Fs->code_heap)`; pass 10 is the only writer. `IC_CALL` becomes a 5-byte `E8 rel32`; targets beyond 2 GiB fall back to `MOV RBX,imm64` plus `CALL RBX`. https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Compiler/PrsLib.HC, https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Compiler/OptPass789A.HC
- Intermediate code is "rev polish stack machine in nature" (`PassTrace()`); JIT mode places code and data in heap memory for in-place execution; all code stays below 2 GiB because `CALL` reaches +/-2 GiB; AOT (`.PRJ` to `.BIN`) is only for the Kernel and Compiler; users are told "Do not use AOT"; a loaded module unloads only by killing its task. https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/Glossary.DD
- Pipeline and size: parser emits stack-machine IR, optimizer passes, back ends `BackA/B/C/FA/FB.HC`, assembler `Asm*.HC` and disassembler `UAsm.HC` built in. `Compiler/` is about 498 KB in 37 files (local count), 18,149 lines of `.HC` and 18,634 with two `.HH` (cloud count); `Compiler.BIN` is 193,792 bytes. Build order: https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Compiler/Compiler.PRJ
- No separate preprocessor: the lexer handles `#include`, `#define`, `#if`, `#ifaot`, `#ifjit`, `#exe`. `#exe{}` runs code at compile time and splices output into the token stream. HolyC has no type-checking and tasks can "MAlloc and Free off-of other task's heaps". https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Compiler/Lex.HC, https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Compiler/PrsStmt.HC, https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/HolyC.DD
- No object files or linker; `Load()` copies a `.BIN` into the code heap and resolves a patch table. https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/FAQ.DD, https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Kernel/KLoad.HC
- `KMain` loads the AOT Compiler and runs `ExeFile("StartOS")`; `/Adam` "is JIT compiled during boot". https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Kernel/KMain.HC, https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/GuideLines.DD
- Symbols resolve in the task's table then its parents'; newer symbols shadow older; duplicates are allowed so a file can be re-included while developing. https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/ScopingLinkage.DD, https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Kernel/KHashA.HC. `Adam("...")` runs source system-wide. https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Kernel/Job.HC
- Redefinition: `PrsFunJoin` reuses an entry only if it is in the current table and still extern; otherwise a new entry shadows the old one. A call to an already-defined function compiles to a direct `IC_CALL`; only extern calls use `IC_CALL_INDIRECT2` through `&exe_addr` (research reading, cross-checked with the optimizer source). `Compiler/PrsStmt.HC`, https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Compiler/PrsExp.HC (around lines 556-571 per the local pass)
- Tools: `Dbg()`, Ctrl-Alt-D, `U()`/`Uf()`, `Trace()`, `PassTrace()`, `Dr()`. https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/DbgOverview.DD. A fault in any task goes to `Fault2`, enters single-user mode and the debugger; no isolation contains it. https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Kernel/KDbg.HC
- Assumptions our decisions exclude:
  - Charter: "Ring-0-only", "Single-address-map". https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/Charter.DD
  - Identity-mapped, one page table for all tasks. https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/MemOverview.DD
  - EFER gets only LME (no NXE) and CR4 gets `0xB0` (no SMEP). https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Kernel/KStart64.HC
  - Every large page is `0x87` (present, writable, user, page-size), so XD is clear. https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Kernel/Mem/PageTables.HC
  - Cooperative scheduling: "A task runs until it voluntarily yields". https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Kernel/Sched.HC
  - "One language and compiler for command-line, scripts, songs, automations and code." https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/Strategy.DD
  - The source "Only compiles with the included TempleOS compiler and assembler". https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/Features.DD

### 2. ZealOS and other HolyC implementations

- **ZealOS** (https://github.com/Zeal-Operating-System/ZealOS, Unlicense, about 2.4k stars, no releases): "only changes to Terry's HolyC compiler are reformatting, label renames, bugfixes, and additions". https://zeal-operating-system.github.io/Doc/ZealC.DD.html. Renames (Prs to Parse, `.BIN` to `.ZXE` 2022-01-28, CosmiC to ZealC 2021-12-11); the only functional compiler changes are assembler additions in October 2021. https://raw.githubusercontent.com/Zeal-Operating-System/ZealOS/master/src/Doc/ChangeLog.DD. The latest commit touching `src/Compiler` is 2023-11-12. https://github.com/Zeal-Operating-System/ZealOS/commits/master/src/Compiler. It keeps ring 0 and `MAlloc(code_size, Fs->code_heap)`. https://raw.githubusercontent.com/Zeal-Operating-System/ZealOS/master/src/Doc/Charter.DD and https://raw.githubusercontent.com/Zeal-Operating-System/ZealOS/master/src/Kernel/KStart64.ZC. Last master commit 2025-11-10 (checked via the GitHub API; the cloud pass's other date of 2025-11-09 is wrong).
- **holyc-lang** (https://github.com/Jamesbarford/holyc-lang, BSD-2-Clause, last commit 2026-09-06): HolyC compiler in C (AST, SSA IR, x86-64 and AArch64), JIT, REPL, LSP; README: "more for fun than for serious software development". https://raw.githubusercontent.com/Jamesbarford/holyc-lang/main/README.md (BSD-style licence: https://raw.githubusercontent.com/Jamesbarford/holyc-lang/main/COPYING). JIT maps read-write, copies, then `mprotect`s read-execute (Apple Silicon: `MAP_JIT`). https://raw.githubusercontent.com/Jamesbarford/holyc-lang/main/src/asm/asm_jit.c. "A redefinition overwrites: future chunks bind to the newest body, already-patched code keeps the old one." https://raw.githubusercontent.com/Jamesbarford/holyc-lang/main/src/jit-common.c. Began 2024-01-29; assembler and JIT landed 2026-06-20 as 30,522 inserted lines. https://github.com/Jamesbarford/holyc-lang/commit/d4ed4ec958b4877c71b2ba33a27f868e952b6213
- **Aiwnios** (https://github.com/aiwnios/Aiwnios): HolyC on amd64, aarch64 and riscv64. The repository is archived (GitHub API, 2026-10-14) with last push 2026-02-02. The cloud pass dates the archiving 2026-09-29 (https://github.com/nrootconauto/Aiwnios/commits/main), which the API check did not confirm; the local pass's "archived 2026-02-02" mislabeled the last-push date.
- **Shrine** (https://github.com/minexew/Shrine): archived 2024-04-20, last commit 2020-11-13. **TinkerOS** (https://github.com/tinkeros/TinkerOS): active, 5.22 on 2026-10-12.

### 3. Language runtimes in `no_std`

**Rhai.** https://github.com/rhaiscript/rhai
- 1.26.1 published 2026-09-10; about 5.7k stars; commits daily (latest 2026-10-13). https://crates.io/api/v1/crates/rhai. Default feature is `std`; `no_std` enables `no-std-compat`, `libm`, `hashbrown`, `core-error`, `no_time`; other features `no_float`, `only_i64`, `grain`, `unchecked`, `sync`. https://docs.rs/crate/rhai/1.26.1/source/Cargo.toml
- Book: "Currently, no_std requires the nightly compiler". https://rhai.rs/book/start/builds/no-std.html. CI job `no_atomic64` runs `cargo check --lib --target riscv32imac-unknown-none-elf --no-default-features --features no_std,grain,sync` on stable; no x86_64 `no_std` job. https://github.com/rhaiscript/rhai/blob/main/.github/workflows/build.yml (raw: https://raw.githubusercontent.com/rhaiscript/rhai/main/.github/workflows/build.yml)
- Speed: 1 million iterations in 0.14 s on a single-core 2.6 GHz Linux VM. https://rhai.rs/book/about/features.html. Grain notes: https://raw.githubusercontent.com/rhaiscript/rhai/main/CHANGELOG.md (checked 2026-10-14, see Conclusion 5).
- `Engine` limits: `set_max_operations` (0 means unlimited, the default), `set_max_call_levels`, `set_max_expr_depths`, string/array/map size, variables, functions, modules; `on_progress` terminates; `set_allow_looping`, `disable_symbol`, `register_custom_syntax`, `compile_with_scope`. None of the limits exist under `unchecked`. https://docs.rs/rhai/1.26.1/rhai/struct.Engine.html. `Engine::new_raw` omits the standard library. https://rhai.rs/book/engine/raw.html
- "Rhai is sand-boxed so a script can never read from outside its own environment"; "Any panic is a bug"; `unchecked` "disables all safety checks (even fatal ones)". https://rhai.rs/book/safety/sandbox.html and https://rhai.rs/book/safety/index.html
- REPL fit: a persistent `Scope` keeps variables; merging each line's functions into a global AST keeps functions (the bundled `rhai-repl` does this but depends on `rustyline`, so `std`). A custom `ModuleResolver` over our VFS makes `import` our include. `gen_fn_signatures` and `definitions()` export the API for LLM context; `metadata` works in `no_std` per the changelog. https://rhai.rs/book/engine/scope.html and https://rhai.rs/book/start/bin.html
- "No formal language grammar": hand-coded lexer, recursive-descent and Pratt parsers. https://rhai.rs/book/about/non-design.html
- `smartstring` is a required dependency; RUSTSEC-2026-0249 says it is unmaintained (repository archived 2026-05-03). https://rustsec.org/advisories/RUSTSEC-2026-0249.html
- Cheat sheet for AI agents: https://raw.githubusercontent.com/rhaiscript/rhai/main/RHAI-CHEATSHEET.md. A full engine compiles to a WASM32 file under 400 KB (no x86_64 `no_std` size published). https://rhai.rs/book/print.html

**Lua, Lisp, Forth, others.**
- No pure-Rust Lua builds for `no_std`: `piccolo` 0.3.3 (2024-06-16) uses `std::io` (https://docs.rs/crate/piccolo/0.3.3/source/src/io.rs), `hematita` 0.1.0 (2021) is GPL-3.0 and `std` (https://docs.rs/crate/hematita/0.1.0/source/src/lua_lib.rs), `luna-core` 4.0.2 uses `std::time`/`std::fs` (https://docs.rs/crate/luna-core/4.0.2/source/src/vm/exec.rs). `mlua` 0.12.2 needs `std` and a C toolchain. https://docs.rs/crate/mlua/0.12.2/source/Cargo.toml
- C Lua 5.5.1 (MIT, about 32,000 lines, 484 KB library; https://www.lua.org/versions.html) routes allocation through `lua_Alloc`, has an instruction-count hook and an EBNF grammar (https://www.lua.org/about.html, https://www.lua.org/manual/5.5/manual.html). It needs `setjmp`/`longjmp` and libc number conversion (https://www.lua.org/source/5.5/ldo.c.html). RFC 2945 says freeing Rust frames with `longjmp` is undefined behaviour. https://raw.githubusercontent.com/rust-lang/rfcs/master/text/2945-c-unwind-abi.md. Lunatik runs Lua in the Linux kernel (https://raw.githubusercontent.com/luainkernel/lunatik/master/README.md); NetBSD's `lua(4)` has an instruction cap `kern.lua.maxcount` (https://man.netbsd.org/lua.4).
- Stak Scheme 0.12.33 (2026-10-14, MIT): `no_std` VM of 1.5 KLOC; `stak-compiler` uses `std::io`; 1.6-2.3x slower than Python 3. https://docs.rs/crate/stak-compiler/0.12.33/source/src/lib.rs, https://raw.githubusercontent.com/raviqqe/stak/main/README.md and https://raviqqe.com/stak/performance. `steel-core` 0.8.3 is active but `std`-only and large; `rust_lisp` and `ketos` are dormant; `blisp` 0.4.7 and `grift_eval` 1.4.0 are little used. https://docs.rs/crate/blisp/0.4.7/source/src/lib.rs
- Rune 0.14.2 (2026-05-22, MIT OR Apache-2.0): `no_std` with `alloc`, instruction budget, 2,771,441-byte crate. https://raw.githubusercontent.com/rune-rs/rune/main/no-std/examples/minimal.rs. Koto, Gluon, Dyon, Mun and Boa need `std`. https://docs.rs/crate/gluon_vm/0.18.4/source/src/channel.rs, https://docs.rs/crate/koto_runtime/0.16.1/source/src/io/stdio.rs, https://docs.rs/crate/dyon/0.51.3/source/Cargo.toml, https://docs.rs/crate/mun_runtime/0.4.0/source/Cargo.toml, https://docs.rs/crate/boa_engine/0.22.0/source/src/lib.rs
- Forth: `forth3` from mnemOS (`no_std`, last pushed 2025-01-21, https://github.com/tosc-rs/mnemos); Forth crates in the `no_std` category are stale. https://crates.io/api/v1/crates?q=forth&category=no-std&per_page=20&sort=downloads
- MicroPython 1.29.0 (2026-08-24, MIT): its `ports/embed` emits self-contained C for embedding; needs a C toolchain and libc shim (local pass; no primary URL recorded).
- Rust itself: rustc needs `std`, an LLVM backend, a POSIX-like environment and `dlopen`. Redox's GSoC 2017 self-hosting work (https://redox-os.org/news/gsoc-self-hosting-final/) reached native rustc and cargo in January 2026. https://www.phoronix.com/news/redox-os-january-2026

**WebAssembly.**
- wasmi 2.0.0 (2026-09-01, MIT/Apache-2.0; https://github.com/wasmi-labs/wasmi): `std` is a default feature, disabling it builds `no_std`; `wat` forces `std`. https://docs.rs/wasmi/2.0.0/wasmi/ and https://docs.rs/crate/wasmi/2.0.0/source/Cargo.toml. `Config::consume_fuel` (`OutOfFuel` trap), `set_max_recursion_depth`, `set_max_stack_height`, `StoreLimitsBuilder` (linear memory unlimited by default), `TypedFunc::call_resumable`; resumable calls since 0.45.0 (2025-05-06). https://docs.rs/wasmi/2.0.0/wasmi/struct.Config.html, https://docs.rs/wasmi/2.0.0/wasmi/struct.StoreLimitsBuilder.html, https://docs.rs/wasmi/2.0.0/wasmi/struct.TypedFunc.html
- wasmi is "for no_std embedded environments", resists JIT bombs, was audited twice, passes the spec suite; 2.0 is about 2.2x faster than 1.0 by geomean on an M2 Pro (CoreMark about 2,800 to over 4,200); fastest dispatch uses unstable `become`. https://raw.githubusercontent.com/wasmi-labs/wasmi/main/README.md and https://wasmi-labs.github.io/blog/posts/wasmi-v2.0/. The v0.32 post says compute-heavy work suits JIT runtimes. https://wasmi-labs.github.io/blog/posts/wasmi-v0.32/
- Munal OS embeds wasmi in its `no_std` kernel as the only barrier between apps and the kernel (it never touches page tables). https://github.com/Askannz/munal-os and https://raw.githubusercontent.com/Askannz/munal-os/master/README.md
- `wasm-encoder` 0.262.0 (2026-10-13) is `no_std` with `alloc`. https://docs.rs/crate/wasm-encoder/0.262.0/source/src/lib.rs and https://docs.rs/crate/wasm-encoder/0.262.0/source/Cargo.toml
- Wasmtime 49.0.3 (2026-10-14; 50.0.0-rc.1 2026-10-05; Apache-2.0 WITH LLVM-exception): `no_std` "does not include Cranelift or Winch", so "no_std platforms must be used in AOT mode where the module is precompiled elsewhere"; Pulley is Cranelift-produced and "will never be as fast as native". https://docs.wasmtime.dev/stability-platform-support.html. Mandatory hooks are `wasmtime_tls_get`/`wasmtime_tls_set`; optional hooks cover virtual memory, native signals, sync primitives, fibers. CI builds `examples/min-platform` for `x86_64-unknown-none`; minimal build is 400-698 KB (figures dated 2024-12-12). https://docs.wasmtime.dev/examples-minimal.html. Whether native AOT code runs without the virtual-memory hooks is unverified.
- Sandbox bugs exist: CVE-2023-26489 (CVSS 9.9), a Cranelift x86-64 codegen bug, let a guest escape linear memory. https://github.com/bytecodealliance/wasmtime/security/advisories/GHSA-ff4p-7xrq-q5r8. An interpreter's trusted code base is smaller.
- Fuel and interruption: Wasmtime offers fuel or epoch interruption (https://docs.wasmtime.dev/examples-interrupting-wasm.html).
- Nobody types Wasm at a prompt. It is the component layer, built from Rust, C or Zig on a host service, or from our own language later.

**Comparison (condensed).**

| Option | `no_std` + `alloc` | Speed vs native | License | LLM writability |
|---|---|---|---|---|
| Custom HolyC-like | Yes (we write it) | about 1-3x slower with an optimizer (unverified) | ours | Poor: no training data |
| Rhai 1.26.1 | Yes, CI type-checked | AST about 100x or more on tight loops; Grain faster | MIT/Apache-2.0 | Medium |
| C Lua plus libc shim | With shim | interpreter class | MIT | High |
| MicroPython 1.29 embed | With shim | interpreter class | MIT | Very high |
| wasmi 2.0.0 | Yes | interpreter class, about 10x (anecdotal) | MIT/Apache-2.0 | Not a language |
| Wasmtime 49 `no_std` | Runtime and Pulley only | Pulley interpreter; native AOT about 1.5-1.9x slower than LLVM (anecdotal) | Apache-2.0 WITH LLVM-exception | Not a language |

Anecdotal speed ratios come from a Hacker News commenter in the Wasmi 2.0 thread. https://news.ycombinator.com/item?id=49521031. Cranelift's README claims code about 2% slower than V8 TurboFan and 14% slower than WAVM. https://raw.githubusercontent.com/bytecodealliance/wasmtime/main/cranelift/README.md

### 4. Native code generation in `no_std`

- `cranelift-codegen` 0.136.3 (2026-10-14): `#![no_std]` plus `alloc`, `std` default feature, 15 required direct dependencies, `.crate` 1,203,783 bytes. https://docs.rs/crate/cranelift-codegen/0.136.3/source/src/lib.rs
- Wasmtime CI runs `cargo check`, not tests, for `x86_64-unknown-none` on `cranelift-assembler-x64`, `cranelift-codegen`, `cranelift-frontend` and `pulley-interpreter`. https://raw.githubusercontent.com/bytecodealliance/wasmtime/main/.github/workflows/main.yml (also https://github.com/bytecodealliance/wasmtime/blob/main/.github/workflows/main.yml). `cranelift-assembler-x64` 0.136.3 is "A Cranelift-specific x64 assembler": https://crates.io/api/v1/crates/cranelift-assembler-x64
- `cranelift-jit` 0.136.3 needs `std`, `libc`, `memmap2`, `region` (and `windows-sys`). https://docs.rs/crate/cranelift-jit/0.136.3/source/Cargo.toml. `cranelift-native` uses `std::is_x86_feature_detected!`. https://docs.rs/crate/cranelift-native/0.136.3/source/src/lib.rs. Whether `cranelift-module` works in `no_std` is unverified.
- `iced-x86` 1.21.0 (2024-01-20, still latest, MIT; repo commit 2026-07-14): `no_std` plus `code_asm`. https://docs.rs/iced-x86/1.21.0/iced_x86/. `dynasmrt` 5.1.0 (2026-07-22, MPL-2.0) needs `memmap2` and `std`. https://docs.rs/crate/dynasmrt/5.1.0/source/src/lib.rs
- Copy-and-patch: code generation two orders of magnitude faster than LLVM -O0, running an order of magnitude faster than interpretation. https://arxiv.org/abs/2011.13127. CPython's copy-and-patch JIT is about 900 lines of build-time Python plus 500 lines of runtime C. https://peps.python.org/pep-0744/
- Under QEMU TCG a guest JIT forces QEMU to discard translated code whenever it writes a code page. https://www.qemu.org/docs/master/devel/tcg.html

### 5. Interpreter performance

- Efficient interpreters run about 10x slower than native; poor ones over 1,000x. https://jilp.org/vol5/v5paper12.pdf
- Stack-to-register translation removes over 47% of VM instructions and cuts run time 26-32%. https://www.usenix.org/legacy/events/vee05/full_papers/p153-yunhe.pdf. Lua 5.0's register VM is more than twice as fast as 4.0's stack VM on a simple loop. https://www.lua.org/doc/jucs05.pdf
- Rust's `x86_64-unknown-none` uses no SSE or AVX by default, so floats are software-emulated. https://raw.githubusercontent.com/rust-lang/rust/master/src/doc/rustc/src/platform-support/x86_64-unknown-none.md and https://doc.rust-lang.org/rustc/platform-support/x86_64-unknown-none.html

### 6. Natural-language layer and constrained decoding

- llama-server takes GBNF as `grammar` on `/completion` and, per source reading, raw on `/v1/chat/completions`; it throws "Cannot use both json_schema and grammar" and "Cannot use custom grammar constraints with tools". https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md, https://raw.githubusercontent.com/ggml-org/llama.cpp/master/tools/server/README.md and https://raw.githubusercontent.com/ggml-org/llama.cpp/master/tools/server/server-common.cpp. Responses report `stop_type` `limit` or `truncated`.
- GBNF: repetition maximum above 2,000 silently becomes unbounded, minimum above 2,000 is an error, left recursion is rejected (https://raw.githubusercontent.com/ggml-org/llama.cpp/master/src/llama-grammar.cpp). `x? x? ...` patterns can make sampling "extremely slow"; use `x{0,N}`; a grammar can match special tokens such as `<think>` to leave a free section. https://github.com/ggml-org/llama.cpp/blob/master/grammars/README.md and https://raw.githubusercontent.com/ggml-org/llama.cpp/master/grammars/README.md. The sample C grammar (about 40 lines) accepts any identifier and unbounded `while` loops. https://raw.githubusercontent.com/ggml-org/llama.cpp/master/grammars/c.gbnf
- LLGuidance (Lark-like, `%llguidance`) costs about 50 microseconds per token mask at 128k vocabulary, is off by default (`-DLLAMA_LLGUIDANCE=ON`). https://github.com/ggml-org/llama.cpp/blob/master/docs/llguidance.md, https://raw.githubusercontent.com/ggml-org/llama.cpp/master/docs/llguidance.md, https://raw.githubusercontent.com/guidance-ai/llguidance/main/docs/syntax.md and https://raw.githubusercontent.com/ggml-org/llama.cpp/master/CMakeLists.txt
- Ollama constrains only through `format` (json or a JSON schema); no grammar field (v0.40.4, 2026-10-12). https://raw.githubusercontent.com/ollama/ollama/main/docs/capabilities/structured-outputs.mdx, https://raw.githubusercontent.com/ollama/ollama/main/docs/api.md, https://raw.githubusercontent.com/ollama/ollama/main/api/types.go and https://raw.githubusercontent.com/ollama/ollama/main/openai/openai.go
- `mlx_lm.server` documents and reads no `grammar`, `json_schema` or `response_format` field, is "not recommended for production" (local pass saw v0.31.3, 2026-04-22; PyPI later shows 0.32.0, 2026-10-01). https://raw.githubusercontent.com/ml-explore/mlx-lm/main/mlx_lm/SERVER.md and https://raw.githubusercontent.com/ml-explore/mlx-lm/main/mlx_lm/server.py
- In-process MLX options: Outlines (JSON Schema, regex, CFG via `from_mlxlm`; v1.3.3, 2026-08-06), XGrammar 0.2.8 (2026-09-24; MLX kernel exists, no first-party mlx-lm integration found), llguidance Metal kernel. LM Studio uses llama.cpp grammars for GGUF and Outlines for MLX; it warns models under 7B often fail structured output (vendor claim). https://dottxt-ai.github.io/outlines/latest/features/models/, https://raw.githubusercontent.com/dottxt-ai/outlines/main/docs/features/models/mlxlm.md, https://raw.githubusercontent.com/mlc-ai/xgrammar/main/python/xgrammar/kernels/__init__.py, https://raw.githubusercontent.com/guidance-ai/llguidance/main/python/llguidance/mlx.py and https://lmstudio.ai/docs/developer/openai-compat/structured-output
- Research:
  - Input-dependent grammars improve structured tasks without fine-tuning. https://arxiv.org/abs/2305.13971
  - SynCode removed 96.07% of syntax errors but "does not support enforcing semantic constraints". https://arxiv.org/html/2403.01632v4
  - Constrained decoding "can distort the LLM's distribution". https://proceedings.neurips.cc/paper_files/paper/2024/hash/2bdc2267c3d7d01523e2e17ac0a754f3-Abstract.html
  - JSONSchemaBench: llama.cpp 0.63 vs 0.13 compliance; about 29.98 ms per token with grammar vs 15.40 ms without (Llama-3.1-8B, A100). https://arxiv.org/html/2501.10868v3
  - XGrammar reports up to 100x faster mask checks. https://arxiv.org/abs/2411.15100
  - Strict formats hurt reasoning. https://aclanthology.org/2024.emnlp-industry.91/
  - Type errors are "beyond the domain of syntax". https://arxiv.org/abs/2504.09246
  - Monitor-guided decoding. https://arxiv.org/abs/2306.10763
  - Constrained models still emit references to undefined names. https://arxiv.org/pdf/2607.18357
  - Recent preprints report strict constraints can derail chain-of-thought (unverified, local pass).
- Plan then constrain: let the model write a short unconstrained plan and constrain only the code block, for example `root ::= plan "```rhai\n" program "```"`.

### 7. LLMs writing unfamiliar languages

- Model skill rises with language share in training data; The Stack has about 64 GB Python vs 0.5 GB Scheme/Racket. https://arxiv.org/abs/2308.09895
- On Cangjie, models under 200B scored near zero; a 2,146-token syntax reference was the best prompt fix. https://arxiv.org/html/2603.14501
- Few-shot moved a 1B model only slightly (13.9 to 14.1 on R). https://arxiv.org/html/2501.19085v1
- Models struggle with languages absent from pre-training; SPEAC has the model write a known intermediate language. https://arxiv.org/abs/2406.03636
- A minimal BNF in the prompt helps DSL generation. https://arxiv.org/abs/2305.19234
- Self-repair gains are "often modest". https://arxiv.org/abs/2306.09896

### 8. Confirmation, injection and agency

- Claude Code users approve 93% of permission prompts. https://anthropic.com/engineering/claude-code-auto-mode. A rule matching command text "isn't a security boundary". https://code.claude.com/docs/en/permissions
- Copilot CLI: approve once, per session or reject; recommends a sandbox. https://docs.github.com/en/copilot/concepts/agents/about-copilot-cli. Open Interpreter should "fail closed rather than silently running unsandboxed". https://www.openinterpreter.com/docs/terminal/sandbox
- OWASP: no fool-proof prompt-injection prevention; treat model output as untrusted; authorize downstream. https://genai.owasp.org/llmrisk/llm01-prompt-injection/, https://genai.owasp.org/llmrisk/llm052025-improper-output-handling/ and https://genai.owasp.org/llmrisk/llm062025-excessive-agency/
- Linux disables unprivileged BPF by default; the closest analogue to untrusted code in ring 0. https://raw.githubusercontent.com/torvalds/linux/master/kernel/bpf/Kconfig
- Halting is undecidable. https://www.cs.virginia.edu/~robins/Turing_Paper_1936.pdf

### 9. Protection hardware and QEMU features

- NXE=0 allows fetch from any supervisor address; NXE=1 blocks fetch from XD pages; CR0.WP keeps read-only pages read-only for ring 0. https://cdrdv2-public.intel.com/929358/325384-093-sdm-vol-3abcd.pdf
- SMEP blocks supervisor fetch from user-accessible pages; SMAP blocks supervisor data access unless EFLAGS.AC. Same SDM, and https://cdrdv2.intel.com/v1/dl/getContent/671190
- At CPL 3 only user pages are accessible; U/S=0 at any level is supervisor-only. After changing paging entries the TLB must be invalidated (shootdown via IPIs on SMP), and code written by one CPU and run by another needs a serializing instruction. Same SDMs.
- UMIP makes SGDT/SIDT/SLDT/SMSW/STR fault at CPL > 0; `IN`, `OUT`, `CLI`, `STI` run only if CPL <= IOPL. https://cdrdv2.intel.com/v1/dl/getContent/671200
- PCIDs are a TLB optimization. SYSRET raises #GP if RCX is not canonical, so check first or use an IST stack. https://cdrdv2.intel.com/v1/dl/getContent/671190. Protection keys control read and write only, not execution.
- Linux counters Meltdown with page-table isolation. https://docs.kernel.org/arch/x86/pti.html
- QEMU TCG masks include SMEP, SMAP, UMIP, PKU, NX, SYSCALL; PCID is missing; `qemu64` has no leaf-7 features; `max` enables all accelerator features. https://gitlab.com/qemu-project/qemu/-/raw/master/target/i386/cpu.c

### 10. Capability models

- Zircon handles carry rights; a new Fuchsia process is empty. https://fuchsia.dev/fuchsia-src/concepts/kernel/handles and https://fuchsia.dev/fuchsia-src/concepts/process/sandboxing
- seL4: a capability is "a unique, unforgeable token". https://docs.sel4.systems/Tutorials/capabilities.html
- Redox sandboxes by limiting namespace schemes; a 2026 post moves the namespace into user space as a capability. https://doc.redox-os.org/book/security.html and https://redox-os.org/news/nlnet-cap-nsmgr-cwd
- Design: the session holds handles such as `Dir("/home", rw)`, `Net("llm-server")`, `Display`, `ProcInfo(read)`; host functions take handles, not ambient paths; the confirm screen is the grant; scripts never get raw memory or port I/O.

### 11. Estimates and analysis

**Measured line counts (physical lines, 2026-10-14).**

| Codebase | Scope | Lines |
|---|---|---|
| TempleOS c26482b | `Compiler/*.HC` | 18,149 |
| TempleOS | `.HC` plus two `.HH` | 18,634 |
| TempleOS by stage | lexer 1,470; parser 3,444; optimizer 4,591; back end 4,094; assembler 1,715; disassembler 682; `CMain` 702 | |
| ZealOS eee210f | `src/Compiler/*.ZC` plus `.HH` | 22,067 |
| holyc-lang 06ca611 | top-level `src/*.c` and `*.h` | 48,266 |

**Effort (one strong Rust developer new to compilers and kernels, part-time).**
- Rhai REPL in a ring-3 host: 2-4 weeks once ring 3, a user heap, console syscalls and a timer exist.
- Custom HolyC-like language (5k-10k lines of Rust): 2-4 months. Template JIT on iced-x86: another 3k-6k lines, 1-3 months. Cranelift glue: 1.5k-3k lines, 1-2 months. Kernel support for W^X JIT pages: 2-3 weeks after SMP and IPIs.
- GBNF for a small Rhai subset (`let`, assignment, calls, `if`/`else`, `for ... in`, literals; no closures, custom syntax or `eval`): 40-120 lines.

**Latency of one LLM-written script (8B 4-bit, 20 tok/s).** Generate 150-400 tokens: 7.5-20 s; up to 15-40 s if grammar overhead matches JSONSchemaBench (different hardware). Grammar mask with llguidance: under 0.2% of a 50 ms token. Parse, scope check and capability derivation: milliseconds. Spawn: microseconds to milliseconds (full TLB flush without PCID). Generation dominates everything the OS adds.

**Confirm screen.** Show the request verbatim, the full source (diff against the previous attempt), the model's explanation, the derived capability list with resource limits, provenance (model, quantization, server, grammar hash, sampling parameters, stop reason) and a flag for every untrusted input in the prompt. Default answer No. Approval covers exactly what is shown, once.

**Log.** Append-only, readable from the REPL: timestamp, prompt, context hashes, model and server identity, grammar hash, raw output, local check result, user decision, granted capabilities, denied syscalls, faults, budget exhaustion, kill reason, exit status, CPU time. Every executed script is also saved as a named, editable source file; the (intent, script, outcome) triples become few-shot and LoRA data.

**Capability list is not enforcement.** If the language allows calling through a value, the derived list can be incomplete. Handle checks in the kernel are ground truth.

**Meltdown.** TCG does not model speculation, so Meltdown-style leaks should not appear under TCG (inference). Real affected Intel hardware needs separate user page tables.

## Recommended path

**M7 prerequisites for the script host (C0).** Own CR3 per process, `SYSCALL` entry and exit with stack switch and the canonical-RCX check, user heap, timer preemption, per-process page and CPU-time quotas, 8-12 capability syscalls. Done when a ring-3 process that faults, spins forever or exhausts memory is killed and the kernel keeps running. Estimate: 4-8 weeks after paging and interrupts. Decide the FPU policy here.

**M8 REPL and LLM path.**
1. **C1 Rhai REPL (2-4 weeks).** `rhai = { version = "=1.26.1", default-features = false, features = ["no_std", "metadata"] }`, `Engine::new_raw`, limits on, persistent `Scope` plus merged global AST, one statement per Enter, answer echoed as TempleOS does. Measure binary size and start-up under TCG and KVM. TempleOS-style helpers: `include("path")`, `edit("fn")`, `reload("module")`.
2. **C2 OS API (1-2 weeks).** `register_fn` wrappers over capability syscalls: console, read-only scheduler and memory metrics, `policy.propose` to a kernel-side validator that checks bounds. A command-sugar layer maps `ls /home` to `ls("/home")`; `?` forces the LLM, `!` forces script mode.
3. **C3 LLM path (3-5 weeks).** Parse-first routing, JSON envelope `{"summary", "capabilities", "script"}` and in-OS parse-and-repair (works on any backend), then the GBNF "LLM profile" on llama-server, cheat-sheet prompt, stop-reason check, restricted-engine re-parse, capability derivation, confirm screen, fresh process with only granted handles, at most two repair rounds, append-only log.
4. Exit: the 50-task benchmark runs end to end with complete logs and meets the fluency bar.

**M9 note.** Rust components compile on the host to `wasm32-unknown-unknown` and are hot-swapped on wasmi; see [live-editable-system.md](./live-editable-system.md).

**Post-POC.**
- **C4:** a strict, statically typed C-like language emitting Wasm with `wasm-encoder`, runnable on wasmi at once (2-4 months part-time, 5k-10k lines).
- **C5:** native code in ring 3 under W^X (template JIT on iced-x86, or Cranelift with our own loader; 1-3 months plus 2-3 weeks of kernel W^X and TLB-shootdown work).
- **Wasmtime:** only if profiling shows wasmi is the bottleneck.
- **LoRA:** fine-tune the local model on the logged triples.

Rules at every step: the kernel never compiles anything; LLM output runs only after confirmation in a fresh process holding only granted handles; every syscall is checked in the kernel; grammar validity is never treated as safety; nothing an LLM writes runs on the scheduler hot path; scripts can only propose policy through the validator.

## Open questions

1. **Can an 8B model write correct Rhai against our API often enough?** Run 50 tasks (read metrics and print a table, propose a time-slice change for one task class) on the Mac mini through llama-server, with and without grammar and cheat sheet, on the dev host. Record parse, check and run success and success after one and two repairs.
2. **How much does a code grammar slow generation on the M4?** Measure tok/s with no grammar, a GBNF of about 100 rules and an LLGuidance build; check whether the available llama.cpp build includes LLGuidance.
3. **Does Rhai 1.26.1 build and run on `x86_64-unknown-none`,** with and without `no_float`, and how large is it? CI only checks riscv32.
4. **What happens in Rhai when a function is redefined in a persistent Scope or AST?** If existing callers do not switch, we need our own indirection. Test in hosted Rhai first.
5. **Which Rhai constructs allow indirect calls or `eval`, and can `disable_symbol` or engine options switch them off?** A sound capability list depends on it.
6. **When does Rhai 1.27 ship, and how stable is Grain then?**
7. **How coarse should the capability taxonomy be,** and how do we avoid approval fatigue?
8. **Can Wasmtime run native (non-Pulley) AOT code in `no_std` without the virtual-memory hooks?** Only matters post-POC.
9. **Does Cranelift's `no_std` codegen run correctly on `x86_64-unknown-none`?** CI only type-checks it. Only matters for C5.
10. **Do SMEP, SMAP, UMIP and PCID reach the guest under KVM in WSL2?** Print CPUID leaves 1 and 7 under KVM and under TCG `-cpu max` at M0.
11. **How slow is the interpreter in the VM?** Time the same Rhai and wasmi micro-benchmarks natively, under KVM and under TCG.
12. **Should traffic to the Mac mini be authenticated?** A LAN attacker injecting code that a fatigued user approves defeats confirmation. A shared-secret header is the minimum; see the LLM memo.
13. **Is fine-tuning a small model on our language worth it?** Revisit once the log holds enough confirmed, working scripts.
14. **Should the Rhai dependency on `smartstring` (MPL-2.0+, unmaintained) be accepted until 1.27 ships?** Decide with the licence policy before M8.
