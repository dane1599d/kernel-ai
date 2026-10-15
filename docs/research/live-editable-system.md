# Live-editable system: hot swap, state migration and rollback under ring-3 supervision

**Date:** 2026-10-14

Merged from two independent research passes (local and cloud).

- **Project:** Kernel AI
- **Scope:** how Kernel AI puts any component's source one click away and reloads it without a reboot while keeping privilege rings: Theseus and other prior art, the swap unit, source bundling, state migration, rollback, the LLM's role, what must never be hot-swapped, and a POC architecture. Milestones M0-M10 are defined in [rust-kernel-foundation.md](./rust-kernel-foundation.md).
- **Related memos:** [compiler-in-terminal.md](./compiler-in-terminal.md), [llm-in-a-from-scratch-os.md](./llm-in-a-from-scratch-os.md), [templeos-components.md](./templeos-components.md)

## Conclusions

1. **TempleOS and Theseus get live editing from one address space and one privilege level. Our decisions rule both out, so live editing is built layer by layer.** TempleOS compiles each command into heap memory and re-including a file compiles it again into fresh memory, with no state migration. Theseus credits its single swap mechanism and joint evolution of applications and kernel to its single address space and single privilege level (SAS/SPL), and says the compiler cannot check components in different address spaces or privilege levels.

2. **Use two swap tiers in the POC (founder decisions: kernel is a static binary with no in-kernel linker; live-edited code runs in ring 3 under a restarting supervisor).**
   - **Tier 0, data only (kernel).** Policy parameters and the choice among policies compiled into the kernel. A stable policy hook swaps and drains state from the old policy into the new one, as Theseus's `set_policy` does.
   - **Tier 1, ring 3.** Components run in ring-3 host processes under a supervisor. The swap unit is a plain WebAssembly module on `wasmi` inside such a host. Native ring-3 ELF processes (policy agent, workload generators, LLM client) are replaced whole, PROTEOS-style.
   - Superseded by founder decision: in-kernel `wasmi` (old tier 2) and native kernel modules relinked at runtime, Theseus-style (old tier 3).
   - The POC goal, AI-driven scheduling and memory policy on synthetic workloads, needs only tiers 0 and 1. A ring-3 agent can hold arbitrary logic, be replaced and rolled back, and drive native hot-path policies through bounded parameter blocks.

3. **Theseus is the best design reference but not a code base.** MIT-licensed, last commit 2024-09-22, 28 open pull requests (newest is a 2026-08-01 scheduler PR), toolchain pinned to `nightly-2023-10-27`. In 2023 the maintainer said live evolution "has fallen out of focus". Current `swap_crates` says it makes "no attempt to guarantee correct operation" and calls state-transfer functions through an unchecked cast. Its stack fix-up appears only in fault recovery, and `state_store` contains a panic placeholder where the paper describes serialization.
   - Measured results (OSDI 2020, Intel NUC): channel evolution median downtime 385 microseconds; critical swap steps tens to a few hundred microseconds. Loading new cells took 19.5 ms (ITC channel), 21.3 ms (scheduler) and 65.6 ms (network) in the paper, and 179.6 ms when the thesis split the APIC cell (with a pause under 1.3 ms). The two passes quoted different subsets; all are correct for their case.
   - Fault recovery (corrected): of 800,000 injected faults, 0.083% (665) manifested. Theseus recovered 461 of those (69%): 50 by restarting the task, 411 by reloading cells. It failed on 204 (31%): 94 incomplete unwinding, 62 faults in the unwinder, 30 hangs, 18 failed cell replacements. The local pass wrongly wrote that Theseus recovered 69% of all injected faults. Do not quote 69% as an expectation for Kernel AI: it comes from safe-Rust SAS/SPL code.
   - On 13 IPC-layer faults Theseus recovered 11 and MINIX 3 none.

4. **Native module swapping needs one compiler, one flag set and one build tree. That is why state crosses our swap boundaries as bytes.** Default type layout can change with each compilation; the Rust ABI has no stability guarantee; rustc rejects metadata from another compiler version (E0514); `TypeId` hashes change between releases; symbol hashes depend on the compiler version, source and target directories and `-C metadata`. Generic code is monomorphized into callers, so swapping the defining crate leaves callers unchanged. crABI and `#[export]` are open RFCs. `abi_stable` cannot be used in `no_std`; `stabby` builds in `no_std` but is a third-party layer. Ring-3 processes and Wasm modules avoid all of this.

5. **MINIX 3 is the restart precedent and PROTEOS the live-update-with-rollback precedent. Redox is not.**
   - MINIX 3's reincarnation server pings components, restarts with per-component policy scripts (exponential backoff), and recovered from all 347 induced driver crashes (mean NIC recovery 0.48 s). It recovers only stateless drivers, never character-stream drivers, and the project has been dormant since 2018.
   - PROTEOS replaces a whole process as a transaction: update point at the top of the event loop, state transfer runs in the new process, the old process is untouched until probation passes, hot rollback by switching back. Across 50 real updates it needed 265 lines of custom transfer code, median update time 272 ms, maximum 3,550 ms. Its automation depends on an LLVM pass for C and does not transfer to Rust.
   - Redox drivers are user-space daemons but mainline has no restart: `init` discards child exit status, the spawner only logs failures, and the only reported restart is a 5-second NVMe initialization timeout. A student health monitor sits unmerged in a personal GitLab namespace. Redox has no IOMMU yet. Its latest release 0.9.0 was released 2024-09-07 (GitLab release record; the local pass's two dates, 2024-09-09 and 2025-09-22, are both off, the second being the record's `created_at`).

6. **Swap protocol: stage, shadow test, quiesce, transfer, commit, probation, promote.** Handlers run to completion and never block, so quiescence is at most one call duration. Components export `export_state`/`import_state`/`migrate` and the supervisor owns the routing table. The supervisor restarts hosts with heartbeats, backoff and a restart-intensity cap (for example more than 3 restarts in 30 s escalates to "disabled, needs a human").

7. **State migration is explicit, versioned and runs inside the new version. Rolling back code does not roll back state.**
   - Linux documents that once a live patch changes state, going back may be unsafe; Theseus loses buffered messages when a channel evolves from asynchronous to synchronous. A migration without a reverse is "no automatic rollback".
   - Ksplice: 56 of 64 Linux security patches needed no new code; the rest about 17 lines each, mostly for persistent data structures. Adding new fields beside an unchanged structure (shadow variables) avoids migration.
   - Never copy raw memory between versions. Hand-written or LLM-written migrations are the practical route (PROTEOS automation cost 35% extra memory and is C-specific).
   - Disputed: state format. Local: CBOR with a schema-version header, because the shell can display it, the LLM can read it and migrations are easier to debug. Cloud: `postcard` plus version tag plus forward and reverse `migrate`. Resolution: self-describing CBOR with a schema-version header for the POC. Cloud's own evidence (postcard "is NOT considered a Self Describing Format", compatibility "outside of the scope of the wire format") supports this, and POC state is small. Keep postcard as a later optimization for large state. Verify that the chosen CBOR crate supports `no_std` (not checked by either pass).

8. **Rollback is layered and must work offline and in milliseconds.**
   - Keep the old version resident until probation ends (Erlang current/old, MINIX, PROTEOS, Theseus's crate cache).
   - Fall back to a built-in default on any trap, fuel exhaustion, timeout or stall (sched_ext).
   - Boot a new version only after confirmation (OTP `make_permanent`, MCUboot test swaps, systemd boot counting, Android A/B): a boot manifest marks each version `pending` or `known-good`; a panic or hang while pending boots the known-good set next time. QEMU's `i6300esb` watchdog reboots on a full hang.
   - Mirror the version store to git on the host: every live edit is a commit.
   - Disputed: probation window and versions kept. Local: 30 s or K calls, keep 5 versions. Cloud: N policy epochs on the synthetic workload (N set after measurement), keep active plus one standby. Resolution: keep one resident standby for instant rollback (cloud) and the last 5 versions as stored artifacts for `revert <component> [n]` (local); window 30 s or N epochs, whichever is longer, tuned after the first measurements.

9. **WebAssembly: use plain core modules on wasmi behind a small hand-written export contract (founder decision), with WIT written on paper.**
   - A component cannot export memory and talks only through WIT-typed imports and exports, which is the right swap boundary. wasmi 2.0.0 does not implement the component model (maintainer, 2025-02-11: "not planned nor unplanned"), and the only bridge crate uses `std`.
   - Wasmtime in `no_std` has no Cranelift or Winch. It loads only artifacts precompiled by the identical Wasmtime version, through an `unsafe` deserialize and an unstable embedder platform API. Keep it as a later performance tier. Theseus's own Wasmtime port hit the same wall.
   - WASI 0.3.0 (native async) shipped 2026-06-11; Wasmtime 46 first implemented the final spec. Component Model 1.0 has no date.
   - Contract: `lifecycle@1` with `init`, `export-state`, `import-state(from-schema)`, `health`, `self-test`, plus one service interface per component; all I/O through host-capability imports.

10. **Sandboxed code is fast enough for policy at control-plane rates and too slow for mechanism.** Interpreters run about 10x slower than optimizing JITs and baseline compilers 1.5-3x slower; wasmi is reported 2.5-5.2x slower than baseline JITs and about 10x slower than native (secondary); browser Wasm ran 45-55% slower than native on SPEC; SFI overhead of 20-30% is common. The scheduler's per-decision path and page-fault path stay native tier-0 policies with parameters.

11. **Some components are never hot-swapped.** Every prior system keeps such a list (livepatch cannot patch its own ftrace handler; kpatch skips data structures; MINIX cannot live-update its microkernel and restricts VM; PROTEOS cannot update its kernel message substrate; Erlang restarts for ERTS and Kernel upgrades; Theseus holds GDT/TSS/IDT for the whole run). Under our decisions the whole kernel is on this list; see the table in the Recommendation.

12. **"Source one click away" is cheap if decided now.** Ship per-file DEFLATE-compressed source and DWARF line tables in the image, build the symbol index on the host, and let every printed address, name and fault frame be a link (TempleOS's real lesson). A 50,000-line system is about 0.33-0.37 MB compressed.

13. **The LLM explains and proposes; a human confirms every apply; rollback is mechanical and never waits for the LLM.** At about 20 tokens/s a 1,000-token patch takes about 50 s. On kGym the best 2024 frontier models fixed 0.72% of Linux kernel crashes unassisted and 5.38% with the buggy files given. kpatch requires human review of every live patch. The LLM cannot edit the supervisor, the swap machinery or the kernel, and after a rollback it receives the fault log and explains it.

14. **Kernel panics abort; restartability comes from ring 3.** `x86_64-unknown-none` uses `panic=Abort`. Unwinding-based recovery needs a custom target (nightly `-Zunstable-options` since Rust 1.95) and is not needed when faults are contained in ring 3.

## Decisions

| Decision | Status | Default or choice |
|---|---|---|
| Where live-edited code runs | Decided (founder) | Ring 3 under a restarting supervisor; kernel static, no in-kernel linker for the POC |
| Live-swappable unit | Decided (founder) | Plain WebAssembly modules on wasmi |
| LLM placement | Decided (founder) | Remote control plane only; never on the hot path |
| Swap tiers in the POC | Open | Tier 0 (compiled-in policy slots) plus tier 1 (ring-3 supervised Wasm modules and ELF processes) |
| Interface definition | Open | WIT on paper, hand-bound core-Wasm ABI (`lifecycle@1` plus service interface) |
| State format | Open | CBOR with a schema-version header, forward and reverse `migrate` |
| Probation and auto-rollback | Open | Auto-rollback on trap, fuel exhaustion, timeout, missed heartbeat, stalled task or health breach; window 30 s or N epochs |
| Versions kept | Open | One resident standby plus last 5 stored |
| LLM authority | Open | Propose only; human confirms every apply; changes capped per window; every change records provenance |
| Where live edits compile | Open | Host build service (Rust to `wasm32-unknown-unknown`); location (WSL2 or Windows) decided at M9. No on-box Wasm compiler, since Rhai is interpreted |
| Source of truth for edits | Open | Host git repo; in-VM edits sync back; each apply is a commit |
| Source bundle and index | Open | Per-file DEFLATE bundle, `line-tables-only` DWARF, Universal Ctags first, rust-analyzer SCIP later |
| Panic strategy | Open | Abort |
| Drivers in ring 3 | Open | In-kernel for the POC; at most one ring-3 virtio-console demo after M7 |
| Supervisor and state store placement | Open | One ring-3 supervisor holds registry, job queue and swap logic; state-blob store in a kernel-owned shared region so it survives a supervisor restart; kernel restarts the supervisor from a known-good copy |
| Theseus | Open | Borrow ideas only; no fork |
| Performance tier | Open | Wasmtime AOT only after a component is measured too slow on wasmi |

Superseded by founder decision: Wasm tier hosted in the kernel (old tier 2), native kernel modules and in-kernel linker (old tier 3), a swap orchestrator in ring 0, scheduler policy as a Wasm component in the kernel, a language that compiles to Wasm on-box as the live-edit unit.

## Recommendation: POC architecture

**1. Static kernel (ring 0, rebuild and reboot to change).** Boot, descriptor tables, interrupts, paging, allocators, timer, context switch and scheduler mechanism with compiled-in `SchedPolicy` and `ReclaimPolicy` slots, drivers, TCP/IP stack, socket and capability syscalls, panic and fallback console, kernel-owned state-blob region, embedded source tarball and DWARF. Tier-0 policy swap drains the run queue into the new policy under the lock; a stall watchdog restores the default; parameter blocks go through stage/commit/revert.

**2. Supervisor (ring 3, restarted by the kernel).** Component registry (id, interface version, active and standby version, build id, source entry), swap state machine, health checks, append-only commit log, heartbeats and restart policy.

| Step | What happens | If it fails |
|---|---|---|
| Stage | New artifact (Wasm module or ELF) arrives; check build id, interface version, module validation | Reject; nothing changed |
| Prepare | Old instance reaches its update point and exports state, with a timeout | Abort; old keeps running |
| Transfer | New instance imports state through `migrate(old_version, bytes)`; old is suspended, not freed | Kill new, resume old |
| Probation | New serves while health checks run (no traps, no stalled task, deadline misses and reclaim failures in bounds, heartbeats answered) | Roll back: reverse-migrate if possible, else resume old from last exported state, else re-initialize old; log |
| Commit | Free the old instance or keep it cached; mark version "confirmed" | Commit does not fail |

**3. Tier-1 components.** Shell commands, terminal widgets and layout, AI tools (explain, edit, apply, revert), one stateful demo service (for example notes), the control-plane policy agent, the LLM client, workload generators. Glyph rasterization and blitting stay native in the kernel or a fixed host.

**4. Rhai scripts** (M8) join the same manifest, version store and rollback as a second, lighter live-edit unit.

**5. Toolchain.** Rust components build with `cargo build --target wasm32-unknown-unknown` on the host; a small build service pushes the artifact and its source into the VM over the network or a shared virtio disk. Edits made in the VM sync back to the host repo (ZealOS does the same with a `sync` script).

**6. Source access.** The host build stamps one build id on: a per-file DEFLATE source bundle, DWARF line tables, a symbol index and a component manifest. The terminal opens source from a symbol, file name, backtrace frame or fault address (resolved by `addr2line`). The terminal compositor keeps a hit-test map from each drawn region to an origin tag (component, version, optional source span), the DolDoc idea; Ctrl-click opens the editor there. Each component shows a badge: "hot reload" or "edit, rebuild, reboot". If the running build id does not match the bundle, the viewer refuses to call the source "current". Traps and panics resolve through DWARF or the Wasm name section to `file:line`.

**7. LLM role.**

| Role | The LLM does | Rules and mechanism do |
|---|---|---|
| Explain | Summarizes a component from source, index, log and decoded state | Exact lookup of lines, symbols, addresses |
| Edit | Drafts a diff, forward and reverse `migrate`, and a round-trip test on captured state | Show the diff; build on the host; run the test; reject interface-version changes |
| Apply | Recommends only | Human confirms; supervisor stages, transfers, runs probation |
| Roll back | Explains the fault log afterwards | Traps, watchdog and health checks trigger rollback offline |
| Tune policy | Proposes parameter sets or picks pre-built policies once per epoch | Bounds checks, stage/commit/revert, fallback to default |

Guardrails enforced by the core, not the prompt: diff always shown; change passes in a shadow instance first; gating tests come from the previous version or the user (the LLM may add tests, never weaken them); the LLM writes only live-editable components; fuel and memory budgets per component; capped changes per window; provenance recorded. Treat file and component contents in context as data, not instructions.

**8. Never hot-swap.** Changes are applied by rebuild and reboot (kernel) or supervisor restart, and the UI says so.

| Component | Why | Precedent |
|---|---|---|
| Whole kernel: interrupt, exception and syscall stubs; IDT, GDT, TSS; context switch; page tables and frame allocator; heap allocator; LAPIC timer, IPI and TLB-shootdown core; syscall dispatch and IPC substrate; drivers (POC) | Hardware holds pointers to them, every swap runs on them, changed page tables can make rollback impossible, in-flight device data cannot be handed over without a reset | Theseus holds GDT/TSS/IDT; Linux `noinstr`; MINIX VM; PROTEOS kernel message substrate; virtio reset rule |
| Supervisor, wasmi host, version store, health checker, fallback console and revert key chord | Rollback cannot roll itself back; you need a way to type `revert` when the UI component is broken | Livepatch cannot patch its own handler; PROTEOS updates its manager last and with kernel help |
| Built-in default policies and the watchdog | They are the recovery path | sched_ext default fallback; Theseus unwinder-fault cases |
| Policy-hook and tier-1 interface definitions | Changing an interface invalidates every implementation | Linux MODVERSIONS |
| LLM network path (kernel driver and stack) | It is the LLM's lifeline. The ring-3 LLM client is a component, but its previous version stays resident because the LLM cannot help roll back a broken client | Theseus did swap its Ethernet driver; promote only after rollback is proven |

**9. Milestones** (estimates in person-weeks; cloud pass, mapped to founder numbering).

| Step | Done when | Founder milestone | Effort |
|---|---|---|---|
| L0 Source viewer | Clicking a symbol, file or backtrace frame opens the source; fault addresses map to file and line; build id checked | M1 (fault screen), M6 (terminal viewer) | 2-3 |
| L1 Tier 0 policy slots | Scheduler and reclaim policy swap with drain and watchdog fallback; commit log | M4, M10 | 2-4 |
| L2 Supervisor and tier 1 | Heartbeats, backoff, restart cap; process replacement at the update point; state handoff; rollback by resuming the old process. Demo: swap the policy agent and a workload generator under load | M7, M9 | 3-5 |
| L3 Host edit loop | In-VM edits sync to the host repo; host builds; artifact returns over the network; each apply is a commit; a version is used at boot only after confirmation | M6, M9 | 2-3 |
| L4 LLM assist | Explain, propose a diff, draft `migrate`; confirm UI | M9 | 2-3 |

L0-L4 total about 11-18 person-weeks on top of the kernel ladder. M9 exit tests (local): edit a shell command and reload it in under 1 s; change a state schema and the LLM writes the migration while data survives; ship a deliberately trapping version and see automatic rollback; Ctrl-click any UI element to open its source. Add: kill a component and see the supervisor restart it, and hit the restart cap.

## Evidence

### 1. TempleOS and ZealOS baseline

Files from https://github.com/cia-foundation/TempleOS (`archive` branch), base `https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/`.
- Charter: "Ring-0-only", "Single-address-map as though paging is not used", all functions accessible. https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/Charter.DD
- "never switches privilege levels, never changes address maps"; the command line feeds the compiler line by line into `MAlloc`ed memory; `<CTRL-SHIFT-F1>` follows a symbol to source; left-click a file name edits it. https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/Welcome.DD
- All source ships in the distro; Kernel and compiler are built with `BootHDIns()`, the rest JIT-compiled at boot; no object files or linker; "If you #include a file twice, it allocs more mem for it." https://raw.githubusercontent.com/cia-foundation/TempleOS/archive/Doc/FAQ.DD
- ZealOS changes are made "inside the OS" and merged to the repo by a `sync` script after power-off. https://raw.githubusercontent.com/Zeal-Operating-System/ZealOS/master/README.md

### 2. Theseus (design reference)

- Paper: Boos, Liyanage, Ijaz, Zhong, "Theseus: an Experiment in Operating System Structure and State Management", OSDI 2020 (2020-11-04). https://www.usenix.org/system/files/osdi20-boos.pdf and https://www.usenix.org/conference/osdi20/presentation/boos. About 38,000 lines of Rust, 900 lines of bootstrap assembly, 246 crates (176 first-party), 72 `unsafe` blocks in 21 crates, about four person-years.
- Cells: a crate at implementation time, one object file at compile time, loaded sections with bounds and dependency metadata at runtime; "no exemptions for components within a 'base kernel' image"; the statically linked `nano_core` replaces itself by loading its cells one by one; dependencies come from ELF `.rela.*` sections; many small crates because module bounds are lost in an object file (§3.1-3.2). Theseus Book: https://www.theseus-os.com/Theseus/book/design/idea.html and https://www.theseus-os.com/Theseus/book/design/design.html
- SAS/SPL: the paper credits it with a uniform swap mechanism and joint evolution of cells from multiple layers; intralingual design assumes one address space, one privilege level and one allocator ("the compiler cannot holistically observe or check ... components that run in different address spaces or privilege levels"); a Linux syscall "loses the type and lifetime information of its arguments" (§3.1, §4.1-4.2). Invariant M.1 needs a one-to-one virtual-to-physical mapping (§4.3.1). Safe Rust is required: "Unsafe languages require hardware protection or dynamic interposition ... (à la SFI)" (§8). Thesis: "the single address space and single privilege level OS environment renders hardware protection facilities useless anyway" (§8.2.2). https://repository.rice.edu/server/api/core/bitstreams/371f7686-a6f2-4416-8d71-8bc947a30fd1/content
- State spill: "one software component harboring changed states as a result of handling an interaction from another component"; handles force a server handle table, "a form of state spill"; Theseus uses opaque exportation (clients own resource objects) and stateless communication; hardware state (GDT, TSS, IDT, interrupt contexts) goes into a `state_db` store; no global list of allocations (§1, §5.1-5.2; thesis §11.5).
- Swap in four stages: load into an isolated namespace, verify dependencies both ways, rewrite relocations, update stack references and transfer state, atomically remove old cells. Evolution is "as easy as committing to its repository" through a build-server manifest. Transfer functions are hand-written with `Into`. If function types are unchanged stack changes apply at once, otherwise the swap waits for quiescence (thesis §8.1). Limits: no support for assembly access to changed structures; automation limited to adjacent versions (thesis §8.1). A cell that never runs again (boot-time device init) is not re-run on swap (thesis §11.6). Unwinding a task to a quiescent point works only for idempotent functions; async-to-sync channel evolution loses buffered data (thesis §9.1.1). Monomorphized generic channels forced a non-generic singleton channel (thesis §9.1.1). `fn` pointers assume static lifetime, so each task owns the cell holding its entry function (thesis §11.9).
- Results: round-robin scheduler replaced by a priority scheduler at runtime; Ethernet driver and update client swapped together; load times 19.5, 21.3 and 65.6 ms (Intel NUC 6i7KYK); APIC split paused under 1.3 ms with 179.6 ms load (thesis Figure 9.1(d), "25+" dependent cells). A context switch takes 0.35 microseconds dynamically linked, 0.34 statically linked (§7.3). Fault recovery numbers in Conclusion 3 (§7.2, Table 1). Recovery downtime in Liyanage's MS thesis: ITC rendezvous 0.475 ms task restart and 8.214 ms cell reload; display rendering about 3.8 s (Table 5.2). https://repository.rice.edu/server/api/core/bitstreams/81f1c44e-b217-4a25-9719-d08186046543/content. The same thesis's text calls the ITC swap "about 5.5 microseconds", conflicting with its own table; the table is used. When reloading a faulty cell Theseus scans all task stacks for pointers into it and assumes the stored cell is untainted (§4.4).
- Current code, read at commit `1fbfe56` (2024-09-22):
  - `Scheduler` trait and `set_policy`: https://raw.githubusercontent.com/theseus-os/Theseus/1fbfe567075a65ed749b6680db1aeb538819c70c/kernel/task/src/scheduler.rs
  - `swap_crates` ("Correctness not guaranteed"; copies `.data`/`.bss`; transfer functions of type `fn(&Arc<CrateNamespace>, &CrateNamespace)` found by symbol name, "FIXME SAFETY: None"; crate cache for near-instant swap back): https://raw.githubusercontent.com/theseus-os/Theseus/1fbfe567075a65ed749b6680db1aeb538819c70c/kernel/crate_swap/src/lib.rs
  - Symbols resolve by full demangled name including hash; fuzzy matching is "potentially dangerous": https://raw.githubusercontent.com/theseus-os/Theseus/1fbfe567075a65ed749b6680db1aeb538819c70c/kernel/mod_mgmt/src/lib.rs
  - Fault-recovery stack rewrite: https://raw.githubusercontent.com/theseus-os/Theseus/1fbfe567075a65ed749b6680db1aeb538819c70c/kernel/fault_crate_swap/src/lib.rs
  - Relocation writer (`R_X86_64_GOTPCREL` commented out), custom target `"code-model": "large"`, `"panic-strategy": "unwind"`: https://raw.githubusercontent.com/theseus-os/Theseus/1fbfe567075a65ed749b6680db1aeb538819c70c/kernel/crate_metadata/src/lib.rs and https://raw.githubusercontent.com/theseus-os/Theseus/1fbfe567075a65ed749b6680db1aeb538819c70c/cfg/x86_64-unknown-theseus.json
  - `codegen-units = 1`, `incremental = false`: https://raw.githubusercontent.com/theseus-os/Theseus/1fbfe567075a65ed749b6680db1aeb538819c70c/Cargo.toml. Toolchain `nightly-2023-10-27`: https://raw.githubusercontent.com/theseus-os/Theseus/1fbfe567075a65ed749b6680db1aeb538819c70c/rust-toolchain.toml
  - `state_store` keyed by `TypeId` with a `panic!` placeholder: https://raw.githubusercontent.com/theseus-os/Theseus/1fbfe567075a65ed749b6680db1aeb538819c70c/kernel/state_store/src/lib.rs
  - WASI interpreter on `wasmi` 0.9.0; Wasmtime test uses `Module::deserialize()` because the compiler is unsupported: https://raw.githubusercontent.com/theseus-os/Theseus/1fbfe567075a65ed749b6680db1aeb538819c70c/kernel/wasi_interpreter/Cargo.toml and https://raw.githubusercontent.com/theseus-os/Theseus/1fbfe567075a65ed749b6680db1aeb538819c70c/applications/test_wasmtime/src/lib.rs
  - Out-of-tree builds need identical environments and flags (`-C code-model=large -C relocation-model=static -Z merge-functions=disabled -Z share-generics=no`): https://www.theseus-os.com/Theseus/book/building/rust_builds_out_of_tree.html
- Status: newest commit "Makefile: add support for building on FreeBSD (#1103)", 2024-09-22, MIT, 28 open PRs (newest #1117, MLFQ scheduler, 2026-08-01). https://github.com/theseus-os/Theseus/commits/theseus_main, https://github.com/theseus-os/Theseus/commit/1fbfe567075a65ed749b6680db1aeb538819c70c, https://github.com/theseus-os/Theseus/pulls, https://raw.githubusercontent.com/theseus-os/Theseus/1fbfe567075a65ed749b6680db1aeb538819c70c/LICENSE-MIT. Maintainer in March 2023: "The build environment needs to be identical" and live evolution "has fallen out of focus lately". https://github.com/theseus-os/Theseus/issues/902

### 3. Rust ABI, layout and toolchain constraints

- "Type layout can be changed with each compilation." https://doc.rust-lang.org/reference/type-layout.html. "The Rust ABI offers no stability guarantees." https://doc.rust-lang.org/reference/items/external-blocks.html. E0514 on foreign compiler versions. https://doc.rust-lang.org/error_codes/E0514.html
- Layout randomization and `-C metadata` hashing. https://doc.rust-lang.org/nightly/unstable-book/compiler-flags/randomize-layout.html and https://doc.rust-lang.org/rustc/codegen-options/index.html. `TypeId` hashes vary between releases. https://doc.rust-lang.org/core/any/struct.TypeId.html. v0 mangling, `#[no_mangle]`, `#[export_name]`. https://doc.rust-lang.org/rustc/symbol-mangling/index.html
- crABI tracking issue 111423 (open, latest comment 2026-07-06) and RFC 3470; `#[export]` RFC 3435 (open). https://github.com/rust-lang/rust/issues/111423, https://github.com/rust-lang/rfcs/pull/3470, https://github.com/rust-lang/rfcs/pull/3435
- `abi_stable` 0.11.3 (2023-10-12) has non-optional `libloading` and `parking_lot`. https://docs.rs/crate/abi_stable/0.11.3/source/Cargo.toml. `stabby` 72.1.16 (2026-07-20, EPL-2.0 OR Apache-2.0) is `no_std` unless `std` or `libloading`. https://docs.rs/crate/stabby/72.1.16/source/src/lib.rs and https://docs.rs/crate/stabby/72.1.16/source/Cargo.toml
- Built-in `x86_64-unknown-none` uses `PanicStrategy::Abort`. https://raw.githubusercontent.com/rust-lang/rust/main/compiler/rustc_target/src/spec/targets/x86_64_unknown_none.rs. Custom target specs need `-Zunstable-options` since Rust 1.95 (https://github.com/rust-lang/rust/pull/151534). https://raw.githubusercontent.com/torvalds/linux/master/rust/Makefile. `unwinding` 0.2.10 (2026-07-15) gives `no_std` unwinding; its `personality` and `panic` features need nightly. https://docs.rs/unwinding/0.2.10/unwinding/
- Subsecond 0.7.10 patches only the "tip" crate, only with `debug_assertions`, and not struct layouts. https://docs.rs/subsecond/latest/subsecond/

### 4. Ring-based restart and live update

- MINIX 3 RS "guards all servers and drivers", pings for heartbeats, republishes endpoints through a data store; recovery policy scripts with binary exponential backoff; "Recovery of stateful components is not used"; core servers too stateful to recover. https://www.usenix.org/system/files/login/articles/618-herder.pdf (;login: Feb 2007; also https://www.usenix.org/publications/login/february-2007-volume-32-number-1/roadmap-failure-resilient-operating-system). A 512 MB `wget` completed while the Ethernet driver was killed every 4 s at 8% slowdown.
- DSN 2007: "Character device drivers cannot be transparently recovered"; deterministic failures cannot be cured; some real NICs needed a BIOS reset; 12,500 injected faults produced 347 crashes, all recovered (over 99% on preliminary real hardware); 0.48 s mean RTL8139 recovery; throughput loss 25% to 1%; updates reuse the recovery path. https://www.minix3.org/docs/jorrit-herder/dsn-dccs07.pdf
- MINIX live update: all services including RS and VM, not the microkernel or user apps; experimental, x86 only; rollback kills the new instance and resumes the old; VM updates are restricted because changed page tables may prevent rollback. https://wiki.minix3.org/doku.php?id=developersguide:liveupdate. Newest main-repo commit 2018-11-14. https://github.com/Stichting-MINIX-Research-Foundation/minix/commits/master
- PROTEOS (ASPLOS 2013): transactional process-level updates; MMU isolation sandboxes state transfer in the new version; update point only at the top of the event loop; hot rollback on timeouts or fatal transfer errors; 50 updates, 265 lines custom transfer code, median 272 ms, maximum 3,550 ms, allocator overhead 6-130%, under 5% macro overhead when replacing a component every 20 s; LLVM link-time pass embeds state metadata; cannot update the kernel message-passing substrate; update manager is last in the transaction. https://download.vusec.net/papers/asplos-2013.pdf. All 50 real MINIX updates deployed; in 4 the first attempt hit a state-transfer bug that was caught and rolled back (local pass reading of the paper). Reaching quiescence took "a few milliseconds in the worst case", and update time grows linearly with state size.
- Redox: drivers are user-space daemons with restricted schemes, and a failing driver "could be ignored or restarted with no impact on the rest of the system" (https://doc.redox-os.org/book/drivers.html, https://doc.redox-os.org/book/why-a-new-os.html, https://gitlab.redox-os.org/redox-os/base/-/raw/main/drivers/README.md). `init` loops on `waitpid` and discards status (newest commit 2026-09-04, https://gitlab.redox-os.org/api/v4/projects/redox-os%2Fbase/repository/commits?path=init&per_page=3); the service spawner only logs failures; the unit scheduler has one job kind, `Start`; `daemon::spawn` is deprecated and unsupervised. https://gitlab.redox-os.org/redox-os/base/-/raw/main/init/src/main.rs, https://gitlab.redox-os.org/redox-os/base/-/raw/main/init/src/service.rs, https://gitlab.redox-os.org/redox-os/base/-/raw/main/init/src/scheduler.rs, https://gitlab.redox-os.org/redox-os/base/-/raw/main/daemon/src/lib.rs. NVMe 5 s init timeout and a Georgia Tech health monitor. https://redox-os.org/news/this-month-250430/. Monitor project created 2025-08-28, last active 2025-10-05. https://gitlab.redox-os.org/api/v4/projects/CharlliePhillips%2Fservice-monitor. Scheme `close` was not called when a process was killed. https://code.redox-os.org/redox-os/kernel/issues/92. No IOMMU yet. https://www.redox-os.org/news/development-priorities-2025-09/. Active through August 2026 (capability namespaces, multithreaded schemes). https://www.redox-os.org/news/this-month-260831/
- Genode: `init` monitors by heartbeat and a bumped `version` attribute forces restart, taking dependents down too; its lead says bus and storage drivers "must ultimately become infallible" (genode-users list, 2016-2023; no URL recorded). seL4 Microkit and LionsOS: static architecture, parent-supervised restart, live PD upgrade only on the roadmap. https://www.trustworthy.systems/projects/microkit and https://arxiv.org/abs/2501.06234

### 5. Other prior art

- **Linux livepatch.** Per-task consistency: switch when no patched function is on the stack (needs reliable stack traces) or at return to user space; stuck tasks get a fake signal every 15 s; "A patch can remain in transition indefinitely"; only traceable functions can be patched; module removal only when unused. https://docs.kernel.org/livepatch/livepatch.html. Shadow variables allocate new data beside parent structures. https://docs.kernel.org/livepatch/shadow-vars.html. State changes may make downgrade unsafe. https://docs.kernel.org/livepatch/system-state.html. `noinstr` entry code. https://docs.kernel.org/core-api/entry.html
- **kpatch** "patches functions, not data"; hardware-init changes "inherently incompatible"; "no substitute for human analysis". https://raw.githubusercontent.com/dynup/kpatch/master/doc/patch-author-guide.md. **Ksplice**: 56 of 64 patches need no new code. https://pdos.csail.mit.edu/papers/ksplice:eurosys.pdf. Module versioning refuses mismatched CRCs. https://docs.kernel.org/kbuild/modules.html
- **K42** (USENIX ATC 2005): object calls go through a translation table; a mediator blocks calls until quiescence using thread-generation counts; 100 live instances swapped in 20 ms; could not change interfaces or swap low-level exception code; no rollback. https://www.usenix.org/event/usenix05/tech/general/full_papers/baumann/baumann_html/
- **Erlang/OTP**: two code variants, current and old; a third load purges the old and kills lingering processes; `soft_purge` refuses instead of killing; `code_change/3` gets `OldVsn` or `{down, Vsn}`; a failed upgrade rolls back; a release not made permanent is dropped on reboot; ERTS, Kernel, STDLIB and SASL upgrades need a restart; a supervisor terminates all children and itself beyond MaxR restarts in MaxT seconds. https://www.erlang.org/doc/system/code_loading.html, https://www.erlang.org/doc/apps/stdlib/gen_server.html, https://www.erlang.org/doc/system/release_handling.html, https://www.erlang.org/doc/apps/sasl/appup.html, https://www.erlang.org/doc/system/sup_princ.html
- **Smalltalk and Lisp machines**: Smalltalk migrates live instances on class change (dropped fields lost, new fields nil unless a hook exists); Genera patched running worlds. https://en.wikipedia.org/wiki/Genera_(operating_system) (jump-to-source details from memory, unverified).
- **sched_ext** restores the default scheduler on error, stall or SysRq-S. https://docs.kernel.org/scheduler/sched-ext.html. Linux soft lockup is 20 s in kernel mode; hard lockup detected by NMI. https://docs.kernel.org/admin-guide/lockup-watchdogs.html

### 6. WebAssembly as the swap unit

- A component "may not export a memory"; WIT uses a Canonical ABI; WIT has `list<T>`, results and resources, enough for save/load/migrate. https://component-model.bytecodealliance.org/design/why-component-model.html and https://component-model.bytecodealliance.org/design/wit.html
- Wasmtime 49.0.3 (2026-10-14) `no_std`: subset only, "does not include Cranelift or Winch", AOT only, `wasmtime-platform.h` "not guaranteed to be stable"; supports `runtime`, `component-model`, `pulley`, `async`. https://docs.wasmtime.dev/stability-platform-support.html. `Module::deserialize` accepts only the same version, is `unsafe`, and should not see untrusted input; `Component::new` needs `cranelift` or `winch`. https://docs.rs/wasmtime/49.0.3/wasmtime/struct.Module.html, https://docs.rs/wasmtime/49.0.3/wasmtime/component/struct.Component.html and https://docs.rs/wasmtime/latest/wasmtime/component/struct.Component.html (`Component::deserialize` loads native code or Pulley bytecode that needs no executable memory). Instantiation fell from about 2 ms to 5 microseconds with copy-on-write and lazy init on Linux; that needs virtual-memory support we would build. https://bytecodealliance.org/articles/wasmtime-10-performance
- wasmi 2.0.0 (2026-09-01): `no_std`, fuel metering ("Once an execution runs out of fuel a TrapCode::OutOfFuel trap is raised"), no mention of the component model in the changelog. https://raw.githubusercontent.com/wasmi-labs/wasmi/main/README.md, https://raw.githubusercontent.com/wasmi-labs/wasmi/main/CHANGELOG.md and https://docs.rs/wasmi/2.0.0/wasmi/struct.Config.html. Issue "Implement the Wasm component-model proposal" open since 2024-01-16 (repo https://github.com/wasmi-labs/wasmi). https://github.com/wasmi-labs/wasmi/issues/897. `wasm_component_layer` 0.1.18 (2024-08-02) imports `std`. https://raw.githubusercontent.com/DouglasDwyer/wasm_component_layer/master/src/lib.rs. Benchmarks discussion: https://github.com/wasmi-labs/wasmi-benchmarks (approximate, secondary).
- WASI 0.3.0 shipped 2026-06-11. https://wasi.dev/releases/wasi-p3. Cranelift output about 1.5-1.9x slower than LLVM (secondary); its README says about 14% slower than WAVM. https://cranelift.dev/
- Performance: Wasm ran 45-55% slower than native on SPEC in browsers (https://www.usenix.org/conference/atc19/presentation/jangda); interpreters about 10x slower than JITs, baseline compilers 1.5-3x (https://arxiv.org/pdf/2205.01183); SFI overheads of 20-30% are common (https://cseweb.ucsd.edu/~dstefan/pubs/narayan:2025:segue.pdf).
- Wasm covers policy, services, parsers, UI logic and commands. Drivers would need slow host I/O capabilities and DMA can bypass the sandbox without an IOMMU; nothing needing raw kernel pointers or interrupt paths.

### 7. State serialization, rollback, watchdogs, device reset

- "Postcard is NOT considered a 'Self Describing Format'"; schema-revision compatibility is "outside of the scope of the postcard wire format". `postcard` 1.1.3 (2025-07-24). https://postcard.jamesmunns.com/wire-format
- Android A/B: a failed boot returns to the old partition; `markBootSuccessful()` after checks. https://source.android.com/docs/core/ota/ab. systemd boot counting marks an entry bad at zero tries left and `systemd-bless-boot.service` marks it good. https://systemd.io/AUTOMATIC_BOOT_ASSESSMENT/. MCUboot test swap reverts unless made permanent. https://docs.mcuboot.com/design.html
- QEMU emulates an `i6300esb` watchdog (actions: reset, shutdown, poweroff, inject-nmi, pause, debug, none); `-loadvm` starts from a saved state. https://www.qemu.org/docs/master/system/invocation.html
- VIRTIO 1.3: initialization begins with a reset; a reset device must not touch the queues until re-initialized; per-queue reset and `DEVICE_NEEDS_RESET` added. https://docs.oasis-open.org/virtio/virtio/v1.3/csd01/virtio-v1.3-csd01.html. Without an IOMMU a ring-3 driver's DMA is not hardware-confined.

### 8. Source bundling, debug info and indexing

- `object` 0.40.0 (2026-08-01) is `no_std`; default `compression` feature pulls `std`. https://raw.githubusercontent.com/gimli-rs/object/master/Cargo.toml and https://raw.githubusercontent.com/gimli-rs/object/master/README.md. `goblin` 0.10.7 has a `no_std` mode. https://docs.rs/goblin/0.10.7/goblin/
- `gimli` 0.34.0 builds without `std` (`read` needs `alloc`). https://docs.rs/gimli/0.34.0/gimli/. `addr2line` 0.27.1 is `no_std` with `Context::from_dwarf`; its `loader` feature needs `std` and `memmap2`. https://raw.githubusercontent.com/gimli-rs/addr2line/master/Cargo.toml and https://raw.githubusercontent.com/gimli-rs/addr2line/master/src/lib.rs. `-C debuginfo=line-tables-only` keeps only backtrace info. https://doc.rust-lang.org/rustc/codegen-options/index.html
- `miniz_oxide` 0.9.1 (2026-03-13) builds `no_std` with `alloc`. https://raw.githubusercontent.com/Frommi/miniz_oxide/master/README.md. `lz4_flex` supports only the block format in `no_std`. https://docs.rs/lz4_flex/0.14.0/lz4_flex/. A tarball embedded with `include_bytes!` can be read with `tar-no-std` 0.5.0 (2026-08-13, local pass; no URL recorded).
- rust-analyzer writes a SCIP index with `scip`. https://raw.githubusercontent.com/rust-lang/rust-analyzer/master/crates/rust-analyzer/src/cli/flags.rs and https://raw.githubusercontent.com/sourcegraph/scip/main/README.md. The `scip` crate 0.10.0 depends on `protobuf =3.7.2`, which has no `no_std` support, so convert the index on the host. https://crates.io/api/v1/crates/scip/0.10.0/dependencies and https://raw.githubusercontent.com/stepancheg/rust-protobuf/master/protobuf/src/coded_input_stream/mod.rs. Universal Ctags' Rust parser emits modules, structs, traits, impls, functions, enums, macros and more. https://raw.githubusercontent.com/universal-ctags/ctags/master/parsers/rust.c
- Bundle size (measured 2026-10-14 by compressing every `.rs` file with zlib level 6): `x86_64` 0.15.5 has 33 files, 12,937 lines, 445,663 bytes, per-file DEFLATE 97,130 (4.59x), solid 83,769 (5.32x); `smoltcp` 0.14.0 has 115 files, 60,908 lines, 2,061,241 bytes, per-file 407,282 (5.06x), solid 342,388 (6.02x). Both average about 34 bytes per line. Per-file is the relevant figure. A 50,000-line system is about 1.7 MB raw and 0.33-0.37 MB compressed; 200,000 vendored lines add about 6.8 MB raw, 1.3-1.5 MB compressed. DWARF `line-tables-only` size is unmeasured. https://static.crates.io/crates/x86_64/x86_64-0.15.5.crate and https://static.crates.io/crates/smoltcp/smoltcp-0.14.0.crate

### 9. LLM capability on kernel fixes

- kGym: best model 0.72% unassisted and 5.38% assisted (GPT-3.5 Turbo, GPT-4 Turbo, Claude-3 Sonnet, Gemini-1.5 Pro). https://arxiv.org/pdf/2407.02680

### 10. Estimates and analysis

- **LLM timing.** At 20 tokens/s a 300-token explanation takes about 15 s and a 1,000-token patch about 50 s. Fault-time rollback cannot involve the LLM; an edit loop measured in minutes can.
- **Swap latency under QEMU.** No primary measurement exists. Tier 0: microseconds natively. Tier 1: PROTEOS 272 ms median and MINIX 0.48 s NIC recovery are the precedents; assuming TCG slows kernel-heavy code 5-20x (unverified), expect 0.3-3 s per process swap before probation. A Wasm module swap inside one host skips process creation and should be faster (unmeasured). The probation window, not the swap, dominates what the user waits for.
- **Effort assumptions.** Solo founder new to kernels, 30-35 focused hours per person-week, plus or minus 50%, hand-written state transfer. Theseus's loader, relinker and swap code total 5,774 lines at `1fbfe56` (3,241 + 1,430 + 1,103), for scale.
- **Why tiers 0 and 1 suffice** and why no ring-0 code loading is needed: the agent is an ELF or Wasm module built by any compiler, talking to the kernel through syscalls and versioned bytes.

## Open questions

1. **Is wasmi fast enough for terminal UI logic at 60 fps?** Measure a layout pass early. If not, move that code into the native part of the terminal or to Wasmtime AOT.
2. **Does wasmi 2.0.0 build and run on `x86_64-unknown-none` or the ring-3 user target without changes** (does it need its `libm` feature)? Throwaway test under KVM and TCG, with fuel on and off.
3. **How long does a tier-1 swap take under KVM and under icount TCG?** Sets the probation floor. Measure the M9 demo.
4. **Where does the state-blob store live so that it survives a supervisor restart,** and which signals does the kernel use to decide the supervisor is dead?
5. **How should components run on multiple cores?** One serialized instance per component is simplest; an instance per core complicates migration.
6. **How large are `line-tables-only` DWARF sections for the image?** Decides whether line tables ship inside the image or load from a disk. Measure at M1.
7. **What health checks and probation window gate a commit?** Without good gates auto-rollback never fires or fires on noise. Resolve with A/B runs of the synthetic workloads in icount mode, injecting known-bad policies.
8. **After probation commits new writes, are downgrade functions mandatory,** or do we accept losing that window on rollback?
9. **How much real state may go to the LLM for migration tests?** The server is on the LAN, but state may include user data.
10. **Which CBOR crate works in `no_std`,** and is it worth keeping `postcard` for large state?
11. **Where does the host build service run (WSL2 or Windows),** and how does the VM reach it? Under QEMU in WSL2, 10.0.2.2 reaches WSL2's loopback, so a WSL2-hosted service is convenient.
12. **What sync mechanism carries in-VM edits back to the host repo?** Candidates: HTTP push over virtio-net, a shared virtio-blk image, or a serial channel. Prototype the simplest after M6.
13. **Can QEMU snapshots (`-loadvm`) serve as a development-time "undo everything"?** Test `savevm`/`loadvm` under the KVM setup.
14. **Should the POC enable QEMU's emulated IOMMU for any ring-3 driver demo?** Unverified under TCG and KVM-in-WSL2.
15. **How should the supervisor and the Adam-role system service of [templeos-components.md](./templeos-components.md) be split?** Default: one ring-3 supervisor in the POC; split only if sharing fate becomes a problem.
16. **Does a Wasm module that leaks or fragments its linear memory need a hard reset path** beyond restarting the host process?
