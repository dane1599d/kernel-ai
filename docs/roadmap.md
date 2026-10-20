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
- The LLM runs locally, inside Kernel AI, on the CPU: the ring-3 `llm` service runs our own inference engine on a model file attached read-only. This replaced the remote Mac mini on 2026-10-19 (see the Local LLM milestone below).

It is not a gaming OS in the foreseeable future. No from-scratch kernel runs modern games. The POC demonstrates AI-tuned scheduling and memory policy on synthetic workloads inside a VM.

## 2. Architecture

```
 User mode (from M7)
 +--------------------------------------------------------+
 | Terminal + REPL (Rhai, M8)                             |
 | Live-editable WASM (wasmi, M9)                         |
 | Supervisor: restart, rollback                          |
 | llm service: own engine, Qwen3 GGUF, SSE/AVX (L1-L4)   |
 | Policy agent: LLM hints (M10)                          |
 +---------------+----------------------------------------+
                 | syscalls, hint channel, read-only model map
 Kernel (static binary, never hot-swapped, soft-float)
 +---------------+-------------------------------------------+
 | Scheduler mechanism -- SchedPolicy trait (swappable)       |
 |   watchdog: a misbehaving policy is ejected -> fallback    |
 | Memory manager      -- ReclaimPolicy trait (swappable)     |
 | Interrupts, timers, SMP, virtio-net + smoltcp, virtio-blk  |
 | Framebuffer                                                |
 +------------------------------------------------------------+
```

The HTTP backend (`llm=IP:PORT`, an OpenAI-compatible server) stays only as CI's test double and as the temporary path for `script` until L5.

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
| **Local LLM (L1-L7)** | The model runs inside Kernel AI on the CPU: our own engine crates, SIMD in ring 3, a read-only model disk, then `kai> ask` answered by Qwen3-0.6B. Details below. | Each of L1-L4 has its own CI exit test; L4's `local-ask` boot case answers from a model disk with no LLM server. |
| **M10 AI policy demo** | Demand paging with an in-RAM swap target (virtio-blk later, reusing L3's driver), policy agent choosing and tuning pre-built policies from measured workload, A/B harness. Starts no earlier than L4, preferably after L6: a 0.6B model choosing policies is not a credible demo. | On a synthetic interactive-plus-background workload, measured with deterministic TCG icount, the AI-tuned policy beats the default on interactive latency without starving background work, across repeated runs. |

The fluency test in M8 now points at local models. Until L5 it runs against llama-server on the Windows host, CPU only, as a measuring tool for L6's model (Qwen3-4B by the founder's decision, with 1.7B and 8B measured alongside); from L5, `cargo xtask fluency --local ID` runs it through our own engine.

**M9 status:** M9a, the swap, migration, probation and rollback mechanism with prebuilt versions, is built and checked by the `live` boot case and the same scenario run on the host. M9b, the edit loop, is built and checked by the `edit` boot case and the same scenario run on the host first with real builds: a component edited in the OS is built on the host, committed to git, shown and confirmed, and runs live with its state; a compile error changes nothing and a broken edit rolls back by itself. That meets the M9 exit. M9c part 1 is built: `:state`, `ctl try` and `:try`, COMP_CONTROL for the edit service, `:why`, and build requests that say who wrote the source, with model-written source refused by the host. The build sandbox and the LLM draft path are parked. M9d (source navigation and persistence) is under way: panic locations, `:fault`, a PS/2 pointer spike checked by the `pointer` boot case, a session journal that gives a restarted edit service the founder's session back, the boot manifest, console cells, which keep a log line from splitting a typed line, and keys that edit the typed line (arrows, Home, End, Delete, Ctrl-C), both checked by the `terminal` case, are built. A version committed live now survives a reboot: notes boots the last version a boot committed, falling back to 2, and swaps to a newer committed one under probation, checked by the `manifest` and `manifest-good` boot cases.

**Local LLM status:** L1 and L3 are built; L2 (SIMD in ring 3) is in review. L1, the engine on the host: our own no_std crates `gguf`, `cpufeat`, `quant`, `bpe` and `infer` run Qwen3 Q8_0, and `cargo xtask setup` checks the PC, downloads a pinned model with resume and verifies its SHA-256. L3, the model disk: PCI bus 0 is enumerated once for virtio-net and virtio-blk; a read-only virtio-blk driver reads into a staging buffer the HAL no longer bounces; `cargo xtask run --model-disk PATH` attaches a model file read-only by an absolute path in a JSON `-blockdev`; and the kernel copies the `kai-model` disk into frames at boot, which MODEL_MAP (call 28) maps read-only into `llm`. The `blk` and `model` boot cases check it on generated odd-sized fixtures.

**M8 status:** the shell (M8a) and the LLM script path (M8b) are built and pass their CI cases against the mock server. The fluency gate and the 20-request exit test still need a real model; they now run against local models (see above).

**Open follow-up:** M7's exit test is not written yet; the founder will write it (see `docs/decisions.md`). M7 merged with all earlier exit tests passing and the `net` and `terminal` cases running through the ring-3 llm service.

### Local LLM (L1-L7), approved 2026-10-19

The model runs on the CPU inside Kernel AI's ring-3 `llm` service, on our own clean-room engine. The MVP is L1-L4: `kai> ask` answered by Qwen3-0.6B Q8_0 on one thread. All callers keep the existing ask/script/explain/health IPC protocol. The decisions are in `docs/decisions.md`.

| Step | Deliverable | Exit test |
|---|---|---|
| **L1 Engine on the host** | New no_std+alloc crates: `gguf` (zero-copy reader for untrusted files), `cpufeat` and `quant` (Q8_0 only, a baseline tier and an avx2+fma tier chosen at runtime, bit-identical), `bpe` (byte-level BPE, the qwen2 pretokenizer, ChatML with thinking off), `infer` (qwen3 forward pass, f32 KV cache, greedy generation). `cargo xtask setup` checks the PC, recommends a model, downloads it with resume and checks its pinned SHA-256; `cargo xtask model chat\|check\|pin`; `cargo xtask bench quant` measures the Q8_0 kernels on the PC. | CI unit tests: every malformed GGUF fails with its named error; the dispatched quant tier equals the baseline bit for bit; bpe round trips and golden ids; a generated tiny qwen3 model matches a naive f32 reference within 1e-3 and gives the same 32 greedy tokens; the catalog parses, a tampered download is refused, and store paths on C: or in OneDrive are refused. |
| **L2 SIMD in ring 3** | x87, SSE and AVX on every CPU (XCR0 x87\|SSE\|AVX from `cpufeat`), eager per-thread XSAVE/XRSTOR, an init image before a thread's first ring-3 entry. Only `llm` and the test programs move to a hard-float target; shell, supervisor and comphost stay soft-float. The kernel stays soft-float, and xtask checks its ELF for x87/SSE/AVX instructions. | A new `fpu` case: vector state survives preemption, system calls and migration, a new process starts clean, and the case fails if AVX2 was not exercised. `probe` ops 0-2 must work. |
| **L3 Model disk** | PCI bus 0 enumerated once for virtio-net and virtio-blk; a read-only virtio-blk wrapper (the HAL's `share` learns not to bounce a contiguous direct-map buffer); the kernel copies the `kai-model` disk into frames at boot and MODEL_MAP (call 28) maps them read-only for `llm`. xtask attaches the model file with an absolute path (`-blockdev`), since QEMU's drive paths stay relative. | `blk` and `model` cases on odd-sized fixture disks: the guest's hash equals the host's, a restarted `llm` maps the same bytes, and model frames are never freed. |
| **L4 Local ask (MVP)** | `llm` gains a local backend: parse, tokenizer, a KV cache sized to its quota, greedy `ask`. llmproto's types become backend-neutral first, so `:why` also goes local. A slim `:model`/`:setup` in the terminal, and "No AI model is set up. Everything else works; type :setup". The plan line names `burst_ms` and keeps `net`. `cargo xtask run --local ID` attaches a catalog model (`--model` keeps meaning the server's model name). | `local-ask` case under KVM and TCG: the terminal shows the golden answer xtask computed on the host from the same fixture file, and the mock server gets no request. A kill mid-answer remaps the model rather than reloading it. |
| **L5 Scripts local** | JSON-constrained decoding for `{script, summary}`, seeded sampling, a system-prompt KV prefix cache, `cargo xtask fluency --local`. | Constrained decoding of the fixture always yields JSON the script parser accepts; a `local-script` case. |
| **L6 Speed and Qwen3-4B** | Threads in one address space, futex, per-thread burst accounting, AVX-512 with a VNNI tier, Q4_K/Q6_K, batched prefill, f16 KV, 2 MiB read-only model pages in one shared page-table subtree, background model loading. Default model Qwen3-4B-Instruct-2507 Q4_K_M, per the founder's decision; the fluency measurements check it against 1.7B and 8B. | K-quant tiers equal the baseline; a parallel matvec equals the single-thread result on every vCPU; manual 4B parity with llama.cpp. |
| **L7 Setup inside the OS** | A `:model setup` wizard, then in-OS download under its own plan (DNS, TLS in ring 3, and a writable store, which is new work). | Wizard and download boot cases against an xtask HTTPS mock. |

- **First demo** after L1, on the Windows host with no QEMU: `cargo xtask setup`, then `cargo xtask model chat "..."` answers from our own engine crates and prints tokens/s. The in-OS answer comes after L4.
- **Manual founder gate for L1** (needs the 640 MB file, not in CI): `cargo xtask model check` matches llama.cpp's greedy token ids for 3 prompts x 32 tokens and its tokenizer on 50 strings. The llama.cpp ids come from a manual `workflow_dispatch` GitHub Actions job that builds a pinned llama.cpp tag on a runner; no llama.cpp binary is downloaded to the dev PC.
- **A local accelerated VM is a founder gate before the in-OS demo.** WSL2 on the dev PC has no /dev/kvm and no svm flag (checked 2026-10-19), and QEMU is not installed. xtask gains `--accel wsl`, `--mem`/`--cpus` and a printout of the guest's CPU features and XCR0 early; WHPX is a measured fallback, not an assumption.
- **Speeds are estimates.** One thread of the 7700X under KVM: about 10-20 tokens/s for the 0.6B. Treat this as optimistic: NightRun, a hand-tuned AVX2 engine at llama.cpp parity, runs Qwen3-4B Q4_K_M at about 11 tokens/s on 8 cores (`docs/research/llm-in-a-from-scratch-os.md`). TCG is far below 1 token/s, so CI uses generated fixtures. L1's `cargo xtask bench quant` measures the Q8_0 throughput on the dev PC, so the projection is measured before L6 picks a model. The 0.6B proves the path, not the product.
- **M9d's boot to the committed version** keeps its design: the boot manifest lives in git refs on the host and the kernel command line, with no guest disk, so it does not wait for L3.
- **L1 status (2026-10-19):** built. The engine crates (`gguf`, `cpufeat`, `quant`, `bpe`, `infer`) pass their host tests and match a naive f32 reference on generated models; `cargo xtask setup` and `model chat|check|pin` and `bench quant` exist. Measured on the 7700X, one thread, dev build: a model with Qwen3-0.6B's shapes runs at about 25-28 tokens/s. The llama.cpp parity gate waits for the founder: `cargo xtask setup`, then `model check` once the goldens workflow has run on main.

## 4. Testing strategy

- **Every milestone has an automated exit test** that runs in QEMU and reports pass or fail through `isa-debug-exit`.
- **Host-side unit tests** with `cargo test` cover all pure logic: allocators, policy algorithms, parsers. Policies are tested in a host simulator before they run in the kernel.
- **Accelerators:** KVM in WSL2 for daily work, once the dev PC has it (the Local LLM gate above). TCG in CI, for single-stepping with gdb, and for deterministic icount measurement. WHPX is optional; it has open SMP and early-boot bugs, so no exit test depends on it.
- **Fault injection:** a buggy scheduler policy, a crashing or runaway service, and a broken live edit each have a test proving the system survives.
- **LLM evaluation:** the fluency set plus a fixed set of requests with expected scripts, run against local models. It measures parse rate, correctness and latency, and is re-run whenever the model changes. The engine is checked on the host against a naive f32 reference on generated models in CI, and against llama.cpp's greedy ids by hand.
- **Policy A/B:** the same synthetic workload under the default and the AI-tuned policy, in TCG icount mode, reported as relative numbers.

## 5. Risks

| Risk | Mitigation |
|---|---|
| First-kernel learning curve stalls progress | Small milestones with exit tests. blog_os and Asterinas as references. |
| Guest cannot reach a host service (the build service, an LLM server) from QEMU in WSL2 | Spike in M0. Under WSL2, QEMU's usual host address points at WSL2 itself, not Windows. The local model needs no network. |
| Silent XSAVE bugs give wrong answers, not crashes (L2) | The `fpu` case across preemption and migration, the fresh-process check, the probe flip and the kernel float guard. |
| The hand-written pretokenizer differs from Hugging Face on some non-English text (L1) | ASCII is exact; real-vocab goldens and the llama.cpp parity gate catch the rest, by hand. |
| No fast local VM on the dev PC (L4) | The accelerated-VM founder gate above; CI runs the in-OS cases under KVM and TCG with fixtures. |
| A hostile model file | The kernel checks only the magic and version; ring 3 parses everything with the hardened `gguf` crate, and the supervisor restarts `llm` with backoff. |
| Rhai or wasmi does not build for our target | Spike in M0. Fallback for the REPL is MicroPython. Fallback for live editing is WebAssembly only. |
| Small-model scripts are wrong or unsafe | Grammar plus re-parse, show-and-confirm, execution budgets, probation and rollback, and the fluency gate. |
| Rhai's `smartstring` dependency is MPL-2.0 and unmaintained | Licence policy decided by M8; replace or vendor it if needed. |
| WHPX bugs | KVM and TCG are the tested paths. |
| Scope creep toward games or a desktop | Out of scope until M10 is done. |

## 6. Next step after approval

M0 is small and settles the riskiest assumptions: KVM in WSL2, Rhai and wasmi on our target, and the guest reaching the Mac mini. If any spike fails, this roadmap is revised before M1. The M0 decisions in `docs/decisions.md` are confirmed with the founder when M0 starts.
