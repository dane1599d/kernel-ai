# Kernel AI: roadmap

Status: DRAFT for founder review, revision 2 (2026-10-14). Nothing is built until the founder approves this document.

- Decisions, settled and open: `docs/decisions.md`
- Evidence: the five merged memos in `docs/research/`, each merged from two independent research passes

## 1. What we are building

Kernel AI is our own operating system: a from-scratch x86_64 kernel in Rust, inspired by TempleOS, with a local LLM built into how the system is used and tuned.

- The shell is a REPL. Commands, scripts and plain-English requests all go to the same prompt.
- The system is live-editable. Every user-mode component's source is one step away, and a changed component reloads without a reboot.
- Every symbol, address and stack frame links to its source line, the most valuable idea taken from TempleOS.
- The kernel's scheduler and memory reclaim use swappable policies. The LLM selects and tunes policies but never sits in the scheduling path.
- The LLM runs on the Mac mini M4 and is reached over the network.

It is not a gaming OS in the foreseeable future. No from-scratch kernel runs modern games. The POC demonstrates AI-tuned scheduling and memory policy on synthetic workloads inside a VM.

## 2. Architecture

```
 User mode (from M7)                   Mac mini M4 (LAN)
 +-------------------------------+     +----------------------+
 | Terminal + REPL (Rhai, M8)    |<--->| llama-server, 8B Q4  |
 | Live-editable WASM (wasmi, M9)| HTTP| grammar-constrained  |
 | Supervisor: restart, rollback |     +----------------------+
 | Policy agent: LLM hints (M10) |
 +---------------+---------------+
                 | syscalls, hint channel
 Kernel (static binary, never hot-swapped)
 +---------------+-------------------------------------------+
 | Scheduler mechanism -- SchedPolicy trait (swappable)       |
 |   watchdog: a misbehaving policy is ejected -> fallback    |
 | Memory manager      -- ReclaimPolicy trait (swappable)     |
 | Interrupts, timers, SMP, virtio-net + smoltcp, framebuffer |
 +------------------------------------------------------------+
```

- **Policy hooks** are modelled on Linux sched_ext. The kernel owns run queues and context switching, a policy trait decides, and a watchdog with a round-robin fallback makes a bad policy survivable.
- **Hints, not decisions.** The LLM picks pre-built policies and sets bounded parameters. Policies apply them in microseconds.
- **Every LLM action is shown before it runs.** A generated script or policy change is displayed, confirmed, applied, and rolled back automatically if it faults during probation. Rollback never waits for the LLM.
- **Never hot-swapped:** interrupt entry, context switch, page-table switching, the loader, the allocator, the fault path and the supervisor's core. Changing these means rebuild and reboot.

## 3. Milestones

Effort for M0 to M6 comes from the two research passes, which scoped the work differently. The local pass estimated 15-22 full-time weeks. The cloud pass estimated 22-45 person-weeks with a larger scope. Per-milestone figures are in `docs/research/rust-kernel-foundation.md`. M7 onward is estimated at the M6 review.

| Milestone | Deliverable | Exit test |
|---|---|---|
| **M0 Boot and harness** | Pinned nightly, `x86_64-unknown-none`, UEFI boot through Limine, serial log, framebuffer fill, `cargo xtask run/test`, GitHub Actions CI. Spikes: Rhai and wasmi build for our bare-metal target; a guest TCP connection reaches the Mac mini from QEMU in WSL2. | Boots under KVM and TCG and reports pass or fail through `isa-debug-exit`. CI is green. Spike results and boot-to-prompt time are recorded. |
| **M1 CPU and console** | GDT and TSS, IDT and exception handlers, panic backtrace, framebuffer text console with source links. | A breakpoint is caught and resumed. A page fault reports the address. A stack overflow hits the double-fault handler. The fault screen shows the source file and line. |
| **M2 Memory** | Frame allocator, page tables, guard pages, kernel heap, `ReclaimPolicy` trait with a CLOCK default, page-table layout ready for user mode. | A 1M-operation alloc/free stress test shows no leak. Map, unmap and protect tests pass. |
| **M3 Interrupts and time** | ACPI tables, LAPIC and IOAPIC, PS/2 keyboard, calibrated one-shot timer. | Keys echo. A 1 kHz tick stays within 2% over 60 s, measured against the VM clock, under KVM and TCG. |
| **M4 Preemptive scheduling** | Preemptible kernel threads, context switch, `SchedPolicy` trait, round-robin fallback, runtime swap, 200 ms watchdog. | With 8 CPU hogs and 1 interactive thread, key echo stays under 20 ms. A deliberately buggy policy is ejected and the system survives. |
| **M5 SMP** | Multi-core bring-up, per-CPU run queues, IPIs, load balancing. | At `-smp 4`, a 10-minute spawn, exit and sleep storm shows no deadlock or lost task, under KVM and TCG. |
| **M6 Network and terminal** | PCI, virtio-net with polling, smoltcp, graphical terminal with a fixed built-in command set, LLM client as a kernel task. | A typed command reaches the Mac mini LLM and the reply renders. A policy swap from the prompt works. |
| **M7 User mode and supervisor** | Ring-3 processes, syscalls, SMEP, SMAP and UMIP, one supervisor with a kernel-owned state region, LLM client moved to a ring-3 service. | Under `-cpu max`, a crashing service, a runaway process and a memory-hungry process are each killed and restarted without affecting the kernel. |
| **M8 REPL language** | Rhai REPL as the shell. The LLM writes a Rhai script under a grammar, the OS re-parses it, shows it, asks for confirmation, runs it in a fresh sandbox and logs it. | Gate first: the 50-task fluency test passes (70% first try, 90% after one repair). Then 20 scripted requests run correctly, and every generated script parses. |
| **M9 Live editing** | WASM modules with bundled source, in-OS editor, host build service, CBOR state migration, probation with automatic rollback. | A running component is edited live. A deliberately broken edit rolls back by itself. |
| **M10 AI policy demo** | Demand paging with an in-RAM swap target (virtio-blk later), policy agent choosing and tuning pre-built policies from measured workload, A/B harness. | On a synthetic interactive-plus-background workload, measured with deterministic TCG icount, the AI-tuned policy beats the default on interactive latency without starving background work, across repeated runs. |

The fluency test in M8 can run on the dev host at any time, since it needs only the Mac mini. Running it early de-risks the choice of Rhai.

**M9 status:** M9a, the swap, migration, probation and rollback mechanism with prebuilt versions, is built and checked by the `live` boot case and the same scenario run on the host. M9b, the edit loop, is built and checked by the `edit` boot case and the same scenario run on the host first with real builds: a component edited in the OS is built on the host, committed to git, shown and confirmed, and runs live with its state; a compile error changes nothing and a broken edit rolls back by itself. That meets the M9 exit. M9c part 1 is built: `:state`, `ctl try` and `:try`, COMP_CONTROL for the edit service, `:why`, and build requests that say who wrote the source, with model-written source refused by the host. The build sandbox and the LLM draft path are parked. M9d (source navigation and persistence) is under way: panic locations, `:fault`, a PS/2 pointer spike checked by the `pointer` boot case, a session journal that gives a restarted edit service the founder's session back, and the boot manifest are built. A version committed live now survives a reboot: notes boots the last version a boot committed, falling back to 2, and swaps to a newer committed one under probation, checked by the `manifest` and `manifest-good` boot cases.

**M8 status:** the shell (M8a) and the LLM script path (M8b) are built and pass their CI cases against the mock server. The fluency gate and the 20-request exit test still need the real model on the Mac mini.

**Open follow-up:** M7's exit test is not written yet; the founder will write it (see `docs/decisions.md`). M7 merged with all earlier exit tests passing and the `net` and `terminal` cases running through the ring-3 llm service.

## 4. Testing strategy

- **Every milestone has an automated exit test** that runs in QEMU and reports pass or fail through `isa-debug-exit`.
- **Host-side unit tests** with `cargo test` cover all pure logic: allocators, policy algorithms, parsers. Policies are tested in a host simulator before they run in the kernel.
- **Accelerators:** KVM in WSL2 for daily work. TCG in CI, for single-stepping with gdb, and for deterministic icount measurement. WHPX is optional; it has open SMP and early-boot bugs, so no exit test depends on it.
- **Fault injection:** a buggy scheduler policy, a crashing or runaway service, and a broken live edit each have a test proving the system survives.
- **LLM evaluation:** the fluency set plus a fixed set of requests with expected scripts, run against the Mac mini model. It measures parse rate, correctness and latency, and is re-run whenever the model changes.
- **Policy A/B:** the same synthetic workload under the default and the AI-tuned policy, in TCG icount mode, reported as relative numbers.

## 5. Risks

| Risk | Mitigation |
|---|---|
| First-kernel learning curve stalls progress | Small milestones with exit tests. blog_os and Asterinas as references. |
| Guest cannot reach the Mac mini from QEMU in WSL2 | Spike in M0. Under WSL2, QEMU's usual host address points at WSL2 itself, not Windows. |
| Rhai or wasmi does not build for our target | Spike in M0. Fallback for the REPL is MicroPython. Fallback for live editing is WebAssembly only. |
| Small-model scripts are wrong or unsafe | Grammar plus re-parse, show-and-confirm, execution budgets, probation and rollback, and the fluency gate. |
| Rhai's `smartstring` dependency is MPL-2.0 and unmaintained | Licence policy decided by M8; replace or vendor it if needed. |
| WHPX bugs | KVM and TCG are the tested paths. |
| Scope creep toward games or a desktop | Out of scope until M10 is done. |

## 6. Next step after approval

M0 is small and settles the riskiest assumptions: KVM in WSL2, Rhai and wasmi on our target, and the guest reaching the Mac mini. If any spike fails, this roadmap is revised before M1. The M0 decisions in `docs/decisions.md` are confirmed with the founder when M0 starts.
