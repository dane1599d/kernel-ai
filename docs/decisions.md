# Kernel AI: decision log

One place for every project decision. Founder decisions are settled. Open decisions carry a recommended default and the earliest milestone that needs them. Defaults are only recommendations. Each open decision is confirmed with the founder when its milestone starts.

Sources: the five merged memos in `docs/research/`. Milestones are defined in `docs/roadmap.md`.

## Settled by the founder (2026-10-14)

| Area | Decision |
|---|---|
| Product | Our own operating system, a from-scratch x86_64 kernel in Rust, inspired by TempleOS, with a local LLM built in. |
| Kernel base | Our own kernel: Limine bootloader plus rust-osdev crates. Asterinas OSTD is a design reference only; no code is reused. |
| Privilege model | Rings are kept. No single address space. |
| Scripts and live editing | Run in user mode (ring 3) under a restarting supervisor. The kernel stays a static binary with no in-kernel linker. |
| Script runtime | Rhai for the REPL language. Plain WebAssembly modules on wasmi are the live-swappable unit. |
| Shell | Our own framebuffer terminal. |
| LLM placement | Remote only for the POC, on the Mac mini M4 (16 GB). Control plane only: it writes hints, scripts and policy choices, and never sits in the scheduling or reclaim path. |
| Testing | QEMU only. Dev host: AMD Ryzen 7 7700X, RTX 4070 Ti, 32 GB, Windows 11 Home (no Hyper-V). KVM inside WSL2 is verified present. |
| Gaming | A multi-year goal, outside the POC. The POC proves AI-tuned scheduling and memory policy on synthetic workloads. |
| Pace | No fixed schedule. Effort figures measure progress, not deadlines. |
| Process | Nothing is built until the founder approves the roadmap. Each milestone ends when its exit test passes. |

## Open decisions, by the milestone that needs them

| Needed by | Decision | Recommended default |
|---|---|---|
| M0, decided 2026-10-15 | Rust toolchain | `nightly-2026-10-14`, pinned in `rust-toolchain.toml`. The `limine` and `acpi` crates require nightly features. |
| M0, decided 2026-10-15 | Firmware and boot media | UEFI only: OVMF code plus a fresh variable store, Limine 12.9.3 (checksum pinned in xtask), QEMU virtual FAT drive. |
| M0, decided 2026-10-15 | QEMU version | CI uses Ubuntu 24.04's apt QEMU. Locally, whatever `cargo xtask` finds, recorded in the run log. |
| M0, decided 2026-10-15 | Accelerators and CI | CI runs the boot test under both KVM and TCG. Locally, KVM in WSL2 once Windows' Virtual Machine Platform is enabled; TCG on Windows until then. WHPX only on explicit request. |
| M0, decided 2026-10-15 | Code budget, reuse policy, boot metric | No size budget yet; revisit at M2. Borrow ideas only, clean-room. `cargo xtask test` prints boot-to-ready time, firmware included. |
| M0, decided 2026-10-15 | Kernel licence (workspace manifest) | MPL-2.0 in `Cargo.toml` as the working choice. Contributor terms stay open until the first outside contribution. |
| M1, decided 2026-10-16 | Panic strategy | Abort. The panic handler logs the message and a frame-pointer backtrace. |
| M1, decided 2026-10-16 | Framebuffer, font, source links | Limine 32-bit framebuffer; Noto Sans Mono 16 px regular (basic Latin plus the replacement glyph). Source links come from a line table that xtask builds from DWARF and Limine loads as the `kernel.lines` module; format in the `linetable` crate. |
| M2, decided 2026-10-16 | Paging, heap, reclaim | 4-level paging on Limine's tables through the HHDM; bitmap frame allocator (`mm` crate); fixed 32 MiB `talc` heap, pre-mapped, behind a counting wrapper for leak checks; `ReclaimPolicy` trait with a CLOCK default tested on real accessed bits; all 256 kernel-half PML4 slots pre-allocated for future address spaces; kernel stacks get an unmapped guard page. Heap growth is deferred. |
| M3, decided 2026-10-16 | Interrupt controllers and timer | xAPIC (memory-mapped) local APIC and I/O APIC; the legacy PIC stays masked. HPET is the reference clock; a PIT fallback is not implemented. The local APIC timer runs one-shot at 1 kHz, re-armed each tick for an absolute HPET deadline so latency never accumulates. |
| M3, decided 2026-10-16 | ACPI | Own minimal reader (`acpitables` crate: RSDP, XSDT or RSDT, MADT, HPET) instead of the `acpi` crate, whose 6.x handler needs about 25 methods. Revisit if AML is needed. |
| M3, decided 2026-10-16 | Keyboard | PS/2 with scancode set 1 translation, decoded by `pc-keyboard`. The interrupt only queues scancodes; decoding happens outside the handler. Tests type keys through the QEMU monitor. |
| M4, decided 2026-10-16 | Kernel preemption | Kernel threads are preemptible by the timer and by wake-ups, with a preempt count that defers switching (used around memory-manager calls). Interrupt handlers acknowledge the APIC before switching. |
| M4, decided 2026-10-16 | Scheduler policy shape | Mechanism and policy split like sched_ext, in the host-tested `sched` crate: `Core` owns states, sleep, wake and a 200 ms watchdog; a `SchedPolicy` trait (enqueue, dequeue, pick_next, tick, preempt_on_wake, reset) picks threads. Policies are compiled in, allocation-free and swappable at runtime. Default `Interactive` (woken threads first, preempt on wake); fallback `RoundRobin` (10 ms slices) after an ejection. |
| M5, decided 2026-10-16 | Multiprocessing | Limine starts the other CPUs in xAPIC mode; each gets its own GDT, TSS with a guarded double-fault stack, per-CPU data via GS_BASE, and its own scheduler core. Threads go to the least-loaded CPU and an idle-ish CPU pulls work every 4 ticks when another has at least two more threads. Wake-ups for another CPU send a reschedule interrupt. Unmap and protect shoot down other CPUs' TLB entries by interrupt. Exited threads are reaped once their registers are saved, and their stacks are reused. Tests run with 4 CPUs under KVM; under TCG the VM gets at most as many CPUs as the host has cores (2 on private-repo CI runners), because an overcommitted host delays virtual CPUs in ways the kernel cannot control. The 10-minute storm runs in a separate soak workflow. |
| M6 | Virtio interrupts | Polling first, then MSI-X. |
| M6 | LLM transport and client | virtio-net on QEMU user-mode networking, a blocking client, `serde_json`, plain HTTP plus an API key on the LAN. TLS stays out of the kernel. |
| M6 | LLM server | llama.cpp `llama-server`. It is the only server here that accepts a raw grammar, which M8 needs. |
| M6, M7 | Terminal document model and storage | A node tree for documents. A tar initrd plus a RAM file system. |
| M7 | Floating point in ring 3 | Soft-float user target. Rhai built with `no_float`. |
| M7 | Supervisor design | One ring-3 supervisor. A kernel-owned region holds saved state so it survives a supervisor restart. |
| M7 | Shell privilege | Broad for code the founder types. Minimal for LLM-written scripts. |
| M7, M8 | Symbol scope and capabilities | Lookup chain with capability-gated promotion. A coarse capability set. |
| M7 | Debugger | Attach on a ring-3 fault. |
| M8 | Execution engine | Rhai's AST interpreter, then its faster engine once released. |
| M8 | Approval policy | Confirm every LLM-written script. No auto-run, even for read-only scripts. |
| M8 | Script rules | Redefining a function switches callers through a per-symbol slot. Loops off for LLM scripts, which run under execution budgets. Click-to-run links in documents are not allowed. Only HolyC's conveniences are imitated. |
| M8 | Remote model and routing | Qwen3-8B, 4-bit, thinking off. Requests go to rules first, then the model. Only Apache-2.0 or MIT model licences. |
| M8 | Dependency licence policy | Decide whether dependencies must be permissive. Rhai pulls in `smartstring`, which is MPL-2.0 and unmaintained. |
| M8 | Fluency gate | Before building the LLM path, run a 50-task test on the dev host. Pass at 70% or more on the first try and 90% or more after one repair. If it fails, fall back to MicroPython before inventing our own language. |
| M9 | Swap tiers, interfaces, state format | Tier 0 and tier 1 only. Interfaces written as WIT on paper. CBOR for saved state. |
| M9 | Probation and history | 30 s or N epochs of probation. One standby version resident, the last 5 stored. The LLM only proposes. |
| M9 | Build service | Edits compile on the host, in WSL2 or Windows. Git is the source of truth. |
| M10 | LLM authority over policy | Select pre-built policies and set bounded parameters only. |
| M10 | Measurement mode | Single-thread TCG with icount. Report relative numbers only. |
| Before the first outside contribution | Kernel licence and contributor terms | MPL-2.0 for the kernel with a CLA is the leading option, because only a CLA keeps commercial relicensing possible. Get legal advice before any commercial release. |
| After M10 | Own language, native JIT | Deferred. |
