# Rust kernel foundation: Limine plus rust-osdev crates, QEMU-only testing

**Date:** 2026-10-14

Merged from two independent research passes (local and cloud).

- **Project:** Kernel AI
- **Related memos:** [compiler-in-terminal.md](./compiler-in-terminal.md), [live-editable-system.md](./live-editable-system.md), [llm-in-a-from-scratch-os.md](./llm-in-a-from-scratch-os.md), [templeos-components.md](./templeos-components.md)
- **Milestones:** M0 boot and harness, M1 CPU and console, M2 memory, M3 interrupts and time, M4 preemptive scheduling, M5 SMP, M6 network and terminal, M7 user mode and supervisor, M8 Rhai REPL with LLM scripts, M9 live editing with WASM, M10 AI scheduling and memory policy demo. All memos use this numbering. The cloud pass used a different M0-M6 ladder; it is mapped below.

## Conclusions

1. **Boot with Limine and write the mechanism layer ourselves.** Both passes agree.
   - Limine 12.9.3 (2026-10-06, BSD-2-Clause) plus the `limine` crate 0.6.5 (base revision 6) hands over a higher-half kernel, an HHDM, the memory map, a framebuffer, the RSDP, modules, a best-effort TSC frequency and parked APs.
   - That removes the AP real-mode trampoline and early page-table work. `bootloader` 0.11.17 starts no APs and needs nightly plus unstable `bindeps`. `uefi` 0.41 is for writing loaders. A raw UEFI loader costs 3-6 weeks for no product value.
   - Trap 1: from base revision 3 the HHDM does not cover device MMIO. The kernel must map the LAPIC, IOAPIC and PCIe ECAM uncached itself.
   - Trap 2: Limine's own Rust template still pins `limine` 0.5, so translate it to 0.6.

2. **Asterinas OSTD is a design reference only (founder decision).** Borrow the `Scheduler`/`LocalRunQueue` and `GlobalFrameAllocator` shapes. Paste no code: a pasted OSTD file becomes MPL-2.0.
   - The cloud pass's strongest technical argument against OSTD (no ring-0 code loading, no module loader) is now moot, because live editing runs in ring 3.
   - The other reasons still hold: OSTD boots through Multiboot/PVH and has no Limine path, its tooling (`cargo-osdk`) is Linux and a pinned nightly only, its API broke between minor releases, and `inject_scheduler` can be called once.

3. **Pin a dated nightly Rust toolchain.** Stable is not possible today.
   - `limine` 0.6.5 declares `#![feature(ptr_metadata)]`. `acpi` 6.1.1 declares `#![feature(allocator_api)]`.
   - `abi_x86_interrupt` (safe `set_handler_fn`) and `custom_test_frameworks` are unstable.
   - Pin `x86_64` to 0.15.5 (0.16.0-rc.0 changes `OffsetPageTable` and raises MSRV to 1.98). Use a nightly no older than `nightly-2026-07-10`.
   - Naked functions are stable since Rust 1.88 and could replace the x86-interrupt ABI later, but the two crates still force nightly.
   - Our own crates should enable only `abi_x86_interrupt` and `custom_test_frameworks`.
   - Use `x86_64-unknown-none` (prebuilt `core`/`alloc`, no SSE, kernel code model, `panic=abort`). Its soft-float output also sidesteps WHPX's SSE-on-MMIO limit.

4. **Target QEMU `-machine q35` with virtio devices.** `virtio-drivers` speaks only memory-mapped PCI config (ECAM), which exists on x86 QEMU only for q35.
   - Disputed: accelerator hierarchy. Local: KVM inside WSL2 is the daily loop, WHPX is secondary, TCG is for CI and single-stepping. Cloud: TCG is the reference platform and WHPX an optional fast mode, with QEMU installed from MSYS2. Resolution: KVM in WSL2 daily, because the founder verified `/dev/kvm` and the cloud pass planned around a native-Windows QEMU. TCG stays the authoritative mode for determinism (icount) and CI fallback. WHPX is optional and best-effort, because its open bugs hit exactly what a new kernel stresses.
   - Consequence: QEMU runs inside WSL2 Ubuntu, so MSYS2 and Stefan Weil builds are irrelevant. Ubuntu 24.04's archive ships QEMU 8.2.2 (10.2.1 on 26.04), and the cloud findings on QEMU 11.x source may not match it. Pick a QEMU version at M0.

5. **No Hyper-V (founder decision).** Windows 11 Home has no Hyper-V Manager, and Hyper-V Gen 2 would force VMBus drivers instead of virtio (Gen 2 drops PS/2, PIC, PIT and legacy PCI for VMBus synthetic devices, with no virtio and no gdbstub). https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/plan/should-i-create-a-generation-1-or-2-virtual-machine-in-hyper-v

6. **Use 4-level paging over the HHDM.** Limine enters with 4-level paging and NX. CR4.LA57 cannot change once IA-32e paging is on, and 4-level already covers 128 TiB. Keep the level count in one module. Use `x86_64` `OffsetPageTable`, `talc` behind IRQ-disabling `spin` locks, W^X on kernel text, NX on data and guard pages under stacks. Share PML4 entries 256-511 across address spaces. Lay out the kernel half so split kernel/user page tables can be added later for Meltdown-class hardening.

7. **Skip the 8259 PIC. Use LAPIC (x2APIC when CPUID allows) and IOAPIC found through ACPI MADT with `acpi` 6.1.1.** Own the LAPIC/IOAPIC code (about 300 lines): `rust-osdev/apic` is an unpublished 2020 stub and `x2apic` 0.5.0 calls itself experimental and QEMU-tested only.

8. **Design the timer for TCG, which has no TSC-deadline mode, no invariant TSC and no PCID.** Detect every feature with `raw-cpuid`. x2APIC works under TCG and WHPX.
   - Use LAPIC one-shot mode calibrated against HPET, the ACPI PM timer or the PIT (median of repeated 10-50 ms samples). Use TSC-deadline only when CPUID reports it.
   - Disputed: calibration order. Local: Limine's TSC frequency first, then HPET, then PIT. Cloud: Limine's value is a best-effort hint that may be missing. Resolution: cloud. Use Limine's value only as a cross-check.
   - Disputed: tick design. Local: periodic 1 kHz first, tickless after M5. Cloud: one-shot timer armed from the policy's slice, with a slow watchdog tick. Resolution: both use the LAPIC timer in one-shot mode. Start with a fixed 1 kHz re-arm (simple, works everywhere) and make the slice length a policy output after M4.

9. **Split scheduler mechanism from policy, modeled on sched_ext.** Both passes agree.
   - The core owns task states, context switch, per-CPU run queues, IRQ-safe locks, a runnable-task census with timestamps, the idle loop, IPIs, and a fixed high-priority class for kernel-critical threads (input, watchdog, network bottom halves).
   - A swappable `SchedPolicy` trait (select_cpu, enqueue, pick_next, tick, stopping, init/exit_task, set_weight, apply_hints) runs with IRQs off under a time budget. The core validates every returned CPU or task.
   - A watchdog ejects the policy on a stall (runnable task waiting too long), an invalid result, a panic or a blown budget. The core drains tasks into a built-in fallback (weighted round-robin, later EEVDF-lite). A hotkey does the same (our SysRq-S).
   - Runtime swap: quiesce, drain into the fallback, `init` the new policy, re-enqueue.
   - Stall threshold: local suggests 200 ms in debug builds. Cloud suggests 100-500 ms for interactive workloads (16.7 ms frames), and notes sched_ext's 30 s maximum is a server ceiling. Both fit. Start at 200 ms.
   - The LLM never sits in this path. It reads telemetry and writes bounded `SchedHints` or picks and parameterizes pre-built policies. One remote token takes about 41-50 ms. Scheduling decisions take 0.27-5.7 microseconds, so the gap is 10^4 to 10^6.
   - Code reuse rule: reimplement scx ideas only (GPL-2.0). `rbpf` is not a usable safety verifier (its README says "nothing close" to Linux's).

10. **Disputed: kernel preemption model.** Local: timer-preempted kernel threads at M4 (the M4 exit test runs 8 hogs plus an interactive thread). Cloud: preempt only on return to ring 3 plus explicit yield points, with synthetic workloads in ring 3, which avoids lock-ordering bugs in a first kernel. Resolution: preemptible kernel threads at M4, with a preempt-disable counter that blocks switches inside spinlock and IRQ-off sections. The founder milestone order puts preemptive scheduling at M4 and user mode at M7, so ring-3-only preemption would force M7 first. Ring-3 workloads are added at M7 and used for M10.

11. **Pluggable reclaim: fixed mechanism, swappable policy.**
    - Mechanism: watermarks (min/low/high), a CLOCK-style accessed-bit scanner yielding candidates with metadata (owner, age, dirty, pinned, cache vs anonymous), and Linux-style `Shrinker` callbacks for caches (the only reclaimable memory before M7).
    - `ReclaimPolicy` trait: `on_pressure(level, &MemStats) -> ReclaimPlan`, `rank_victims`, `set_hints`. The core validates every plan (no pinned or DMA pages, never below the min watermark), enforces a time budget and falls back to built-in CLOCK.
    - Export a PSI-style stall metric to the control plane. Shape policies like DAMOS schemes (pattern, action, quota, goal, watermarks), with MGLRU's refault feedback and PageFlex's rule of delegating only non-critical policy.
    - Dependency: demand paging and swap need user address spaces (M7) and a block device. Virtio-blk is not in the M6 scope; see the M10 row of the ladder.

12. **SMP is cheap to start and hard to finish.** Limine's MP request replaces INIT-SIPI-SIPI. The hard parts are per-CPU state (`GS_BASE`), reschedule/TLB-shootdown/panic IPIs and TLB shootdown correctness. No tutorial covers them; read Redox, Hermit, Theseus and OSTD.

13. **Devices to M6.** Display: the Limine (GOP) framebuffer with a RAM back buffer and damaged-rectangle blits, `-vga std`. A plain `virtio-gpu-pci` under OVMF gives a Blt-only GOP, so defer virtio-gpu. Keyboard: PS/2 via the emulated i8042 and `pc-keyboard`. Network: `virtio-drivers` plus `smoltcp`. PCI enumeration over ECAM is our code (`pci_types` helps). Poll first, then MSI-X; legacy INTx on q35 needs AML `_PRT` evaluation, which MSI-X avoids.

14. **Test harness ships in M0.** Serial logging on `-serial stdio`; `isa-debug-exit` at `iobase=0xf4` (exit status is `(value<<1)|1`, so 0x10 gives 33 and QEMU can never exit 0); one fresh QEMU process per test with a hard timeout; `-no-reboot -d int,cpu_reset -D qemu.log` for triple faults; `-s -S` with `rust-gdb` and `hbreak` under KVM; host-side `cargo test` for all pure logic (allocators, page-table logic, policy simulator, parsers); a cargo runner (`cargo xtask run/test`) that assembles the EFI directory, picks KVM then TCG and enforces the timeout.

15. **Measure policies in a deterministic mode and report relative numbers.** Single-threaded TCG with `-icount shift=N,sleep=off` and `-rtc clock=vm`. icount and record/replay refuse hardware acceleration (QEMU exits), force one host thread for all vCPUs, and are not cycle-accurate. KVM runs are for functional work only. Without icount, guest time is host time and results depend on translation speed and host load.

16. **CI fits free GitHub-hosted Linux runners.** Use `ubuntu-24.04` with QEMU and OVMF from apt, hard timeouts (for example 120 s), a KVM udev rule if available and an automatic TCG fallback. TCG is authoritative for timing. The 2026 KVM behaviour is confirmed only by a 2024 changelog and Hermit's CI.

17. **Licensing is clean on the recommended set.** All recommended crates are MIT or Apache-2.0 except `smoltcp` (0BSD). Limine is BSD-2. Never copy from Maestro (AGPL-3.0), scx (GPL-2.0), ArceOS or Aero (GPL). MPL-2.0 code (OSTD, Hubris) can only be borrowed file by file and stays MPL. See Decisions for the kernel's own licence.

18. **blog_os is a one-week primer to about M2.** It has no APIC, ACPI, SMP, preemption, user-mode or virtio posts, and its third edition is unpublished drafts. Prose is CC BY-NC; code samples are MIT or Apache.

19. **Disputed: effort to M6 and beyond.** Local: 15-22 full-time weeks (40 h) or 31-45 part-time weeks (15 h) to its M6 (devices and terminal). Cloud: 22-45 person-weeks (30-35 focused hours, plus or minus 50%) to its M6, which also includes scheduler policy, demand paging, swap and workload generators. The scopes differ, so the numbers are not in conflict. Use the ladder below, which keeps both figures per milestone. There is no fixed schedule (founder decision), so report person-weeks only.

20. **Network facts depend on where QEMU runs.** The network memo assumes slirp reaches the Windows host at 10.0.2.2. With QEMU inside WSL2, 10.0.2.2 reaches WSL2's loopback instead, and guest traffic leaves through WSL2's network (it fell back from NAT to "VirtioProxy" on this machine). Check guest-to-Mac-mini TCP reachability at M0.

## Decisions

| Decision | Status | Default or choice |
|---|---|---|
| Kernel foundation | Decided (founder) | Own kernel: Limine plus rust-osdev crates. OSTD is a design reference only, no code reuse |
| Privilege model | Decided (founder) | Keep rings. No single address space |
| Where live-edited code runs | Decided (founder) | Ring 3 under a restarting supervisor. Kernel is a static binary, no in-kernel linker for the POC |
| Test platform and host | Decided (founder) | QEMU only. Host: AMD Ryzen 7 7700X, Windows 11 Home, no Hyper-V. KVM in WSL2 verified present |
| LLM placement | Decided (founder) | Remote only (Mac mini M4, 16 GB). Control plane only, never in scheduling or reclaim |
| Games and schedule | Decided (founder) | Games are a multi-year goal outside the POC. No fixed schedule |
| Rust toolchain | Open | Pinned dated nightly. Stable only after `limine` and `acpi` drop their feature gates |
| Firmware and boot media | Open | UEFI only (OVMF) from a virtual FAT directory holding `BOOTX64.EFI`, `limine.conf`, kernel ELF. No BIOS path |
| QEMU version in WSL2 | Open | Newest that apt or a source build gives. Record it in the repo |
| Daily accelerator | Open | KVM in WSL2 (window via WSLg). TCG for CI, icount and heisenbugs. WHPX optional |
| Measurement mode | Open | Single-threaded TCG icount; relative numbers only |
| Kernel preemption model | Open | Preemptible kernel threads with a preempt-disable counter (needed at M4) |
| Timer model | Open | LAPIC one-shot, calibrated; fixed 1 kHz first, slice-programmed later |
| Paging | Open | 4-level over the HHDM; level count in one module |
| Heap and frame allocators | Open | `talc` heap; bitmap then buddy frame allocator; per-CPU caches at M5 |
| Interrupt controllers | Open | LAPIC/IOAPIC via `acpi`, own code; skip PIC |
| Display and keyboard | Open | Limine GOP framebuffer, `-vga std`; PS/2 i8042. Virtio-gpu and virtio-input later |
| Virtio interrupts | Open | Polling first, then MSI-X |
| Scheduler policy shape | Open | `SchedPolicy` trait, round-robin fallback, watchdog at 200 ms, hotkey eject |
| Reclaim policy shape | Open | `ReclaimPolicy` trait over CLOCK default, PSI-style metric |
| Policy delivery format | Open | Compiled-in Rust trait objects plus bounded parameter blocks. No in-kernel Wasm or BPF |
| Kernel licence and contributor terms | Open | No licence is forced (no OSTD code reused). Counsel before the first outside contribution; MPL-2.0 for the kernel with Apache-2.0 OR MIT for stand-alone crates is the leading option; CLA vs DCO decided with it |
| KVM in CI | Open | Allowed in an extra job; the TCG job is authoritative for timing |

Superseded by founder decision: build on OSTD (Path B), OSTD as fallback track, Hyper-V (Gen 1 or Pro upgrade), the Intel-vs-AMD host question, a time budget and weekly-hours question, in-kernel Theseus-style hot swap, ring-0 live editing.

## Evidence

### 1. Boot protocol and bootloaders

- Limine latest release 12.9.3, 2026-10-06. https://raw.githubusercontent.com/Limine-Bootloader/Limine/trunk/ChangeLog
- Limine is BSD-2-Clause; the protocol spec and header are 0BSD. https://raw.githubusercontent.com/Limine-Bootloader/Limine/trunk/COPYING
- Limine boots Linux, Limine, Multiboot 1/2 and chainload protocols, reads FAT12/16/32 and ISO9660, supports BIOS, UEFI and hybrid ISOs, and ships a Windows `limine` host tool. https://raw.githubusercontent.com/limine-bootloader/limine/trunk/README.md and https://raw.githubusercontent.com/Limine-Bootloader/Limine/trunk/USAGE.md
- x86-64 entry state: long mode with NX when available, kernel at or above `0xffffffff80000000`, IDT undefined, IF clear, no TSS, PICs and IOAPIC entries masked, IOMMUs off, stack of at least 64 KiB, boot services exited on EFI. Requests: memory map, framebuffer (with EDID), paging mode, RSDP, SMBIOS, modules, command line, TSC frequency (best effort, may be missing), MP. 4-level paging is the default; 5-level is opt-in. https://raw.githubusercontent.com/Limine-Bootloader/limine-protocol/trunk/PROTOCOL.md (spec also at https://github.com/limine-bootloader/limine-protocol/blob/trunk/PROTOCOL.md; last commit 2026-09-25)
- MP startup: APs stay parked; the kernel fills `extra_argument`, then atomically writes `goto_address`; the AP jumps there with its info block in RDI. x2APIC can be requested and AP MTRRs match the BSP. Same spec.
- From base revision 3 the HHDM covers only listed memory-map types (write-back, framebuffers write-combining), so device MMIO is not in it (inference from the spec). Same spec.
- `limine` crate 0.6.5 (2026-06-07, MIT OR Apache-2.0) targets base revision 6; the API changed between 0.5.x and 0.6.x. https://crates.io/api/v1/crates/limine and https://raw.githubusercontent.com/robotman2412/limine-boot-rust/main/README.md
- Limine's Rust template still uses `limine = "0.5"` and floating `nightly`. https://raw.githubusercontent.com/Limine-Bootloader/limine-rust-template/trunk/kernel/GNUmakefile (last commit 2026-03-16)
- `bootloader` 0.11.17 (2026-07-27): BIOS and UEFI, nightly plus `bindeps`, no CPU or SMP fields in `BootInfo`; README says experimental. https://raw.githubusercontent.com/rust-osdev/bootloader/main/README.md
- `uefi` 0.41.0 (2026-09-21) is built for UEFI images and loaders. https://raw.githubusercontent.com/rust-osdev/uefi-rs/main/uefi/README.md
- `ovmf-prebuilt` 0.2.9 (2026-06-06) downloads prebuilt OVMF from edk2 tags. https://crates.io/api/v1/crates/ovmf-prebuilt and https://raw.githubusercontent.com/rust-osdev/ovmf-prebuilt/main/README.md
- QEMU dropped the 32-bit edk2 binaries on 2026-09-01, so a later build may lack `edk2-i386-vars.fd`; pin the OVMF build. https://gitlab.com/api/v4/projects/qemu-project%2Fqemu/repository/commits/403337f1

### 2. Toolchain and language features

- `x86_64-unknown-none` is Tier 2, `no_std`, no SSE/AVX by default, kernel code model, prebuilt via rustup since 1.62. https://doc.rust-lang.org/rustc/platform-support/x86_64-unknown-none.html
- Since Rust 1.68, `alloc` failure panics with no handler needed. https://blog.rust-lang.org/2023/03/09/Rust-1.68.0/
- Naked functions are stable since 1.88. https://blog.rust-lang.org/2025/06/26/Rust-1.88.0/
- `abi_x86_interrupt` is unstable (tracking issue 40180, no stabilization without an RFC; latest comment 2026-10-06). https://doc.rust-lang.org/unstable-book/language-features/abi-x86-interrupt.html and https://github.com/rust-lang/rust/issues/40180
- In `x86_64`, `set_handler_fn` needs `abi_x86_interrupt`; otherwise use unsafe `set_handler_addr`. https://docs.rs/x86_64/0.15.5/x86_64/structures/idt/struct.Entry.html
- `build-std`, artifact dependencies and `custom_test_frameworks` are unstable. https://doc.rust-lang.org/cargo/reference/unstable.html and https://doc.rust-lang.org/unstable-book/language-features/custom-test-frameworks.html
- Rust 1.99.0 was announced 2026-10-01. https://blog.rust-lang.org/releases/
- `limine` 0.6.5 `src/lib.rs` line 6 declares `ptr_metadata`; `acpi` 6.1.1 `src/lib.rs` line 35 declares `allocator_api` (checked 2026-10-14). https://docs.rs/crate/limine/0.6.5/source/src/lib.rs and https://docs.rs/crate/acpi/6.1.1/source/src/lib.rs
- `x86_64` 0.15.5 (2026-07-11): `nightly` feature works with nightlies from `nightly-2026-07-10`. 0.16.0-rc.0 (2026-09-24): `OffsetPageTable` becomes an alias (use `from_phys_offset`), MSRV 1.98, `FsBase::write`/`GsBase::write` become unsafe, `nightly` no longer default. https://raw.githubusercontent.com/rust-osdev/x86_64/master/Changelog.md

### 3. rust-osdev and related crates (read from the crates.io API on 2026-10-14)

| Crate | Latest (date) | License | Verdict |
|---|---|---|---|
| `limine` | 0.6.5 (2026-06-07) | MIT OR Apache-2.0 | Use. MP, HHDM, memmap, framebuffer, RSDP, TSC requests |
| `bootloader` | 0.11.17 (2026-07-27) | MIT OR Apache-2.0 | Skip. No AP startup |
| `x86_64` | 0.15.5 (2026-07-11); 0.16.0-rc.0 (2026-09-24) | MIT/Apache-2.0 | Use 0.15.x: page tables, GDT/IDT/TSS, registers |
| `uefi` | 0.41.0 (2026-09-21) | MIT OR Apache-2.0 | Not in the kernel |
| `acpi` | 6.1.1 (2026-03-08) | MIT/Apache-2.0 | Use. MADT, HPET, MCFG, PM timer; AML interpreter included (old `aml` 0.16.4 is from 2023) |
| `x2apic` | 0.5.0 (2025-02-14) | MIT/Apache-2.0 | Optional; low activity |
| `apic` | 0.1.0 (2019) | n/a | Abandoned; the rust-osdev repo is an unpublished 2020 stub |
| `pic8259` | 0.11.0 (2024-03-11) | Apache-2.0/MIT | Unneeded with Limine |
| `linked_list_allocator` | 0.10.6 (2026-04-14) | Apache-2.0/MIT | Fine for a first heap |
| `talc` | 5.1.1 (2026-09-09) | MIT | Recommended heap; built for kernels, `lock_api`, multiple heaps |
| `buddy_system_allocator` | 0.13.0 (2026-03-30) | MIT | Frame allocator option or reference |
| `spin` | 0.12.3 (2026-08-17) | MIT | Use, wrapped in IRQ-disabling locks; repo moved to Codeberg |
| `conquer-once` | 0.4.0 (2023-04-08) | MIT/Apache-2.0 | Stale; `spin::Once` suffices |
| `uart_16550` | 0.8.1 (2026-09-14) | MIT OR Apache-2.0 | Use; expect churn |
| `virtio-drivers` | 0.13.0 (2026-03-03) | MIT | Use (section 8) |
| `raw-cpuid` | 11.6.0 (2025-09-05) | MIT | Use |
| `pc-keyboard` | 0.9.0 (2026-05-01) | MIT OR Apache-2.0 | Use for PS/2 decoding |
| `pci_types` | 0.10.1 (2026-02-03) | MIT/Apache-2.0 | Use; MSI-X capability helpers, table writes are ours |
| `smoltcp` | 0.14.0 (2026-08-17) | 0BSD | Use for TCP/IP |
| `noto-sans-mono-bitmap` | 0.3.2 (2026-01-07) | MIT | Terminal font candidate |

Sources: https://crates.io/api/v1/crates/linked_list_allocator, https://crates.io/api/v1/crates/buddy_system_allocator, https://crates.io/api/v1/crates/raw-cpuid, https://crates.io/api/v1/crates/uart_16550, https://crates.io/api/v1/crates/apic, https://crates.io/api/v1/crates/smoltcp, https://raw.githubusercontent.com/SFBdragon/talc/master/README.md, https://docs.rs/spin/0.12.3/spin/, https://raw.githubusercontent.com/rust-osdev/pci_types/main/README.md, https://raw.githubusercontent.com/rust-osdev/acpi/main/README.md.

- `acpi` 6.1.1 exposes `AcpiPlatform`, `PmTimer`, `InterruptModel` (`Apic`, `IoApic`, `InterruptSourceOverride`), `PciConfigRegions` (MCFG) and `HpetInfo`; the AML interpreter has a `pci_routing` module. https://docs.rs/acpi/6.1.1/acpi/platform/index.html and https://docs.rs/acpi/6.1.1/acpi/aml/index.html
- `rust-osdev/apic` last commit 2020-09-21, no LICENSE. https://raw.githubusercontent.com/rust-osdev/apic/master/Cargo.toml and https://github.com/rust-osdev/apic/commits/master
- `x2apic` "has only been tested in QEMU", status experimental. https://raw.githubusercontent.com/kzhao0986/x2apic-rs/master/README.md

### 4. Memory management

- Frames: a bitmap over Limine `USABLE` regions first, then a buddy allocator (virtqueues need contiguous aligned DMA memory), per-CPU caches at M5. Reclaim `BOOTLOADER_RECLAIMABLE` only after Limine's responses are no longer read.
- Heap: `talc` as `#[global_allocator]` on a growable frame-backed region.
- CR4.LA57 selects 4- or 5-level paging and cannot change in IA-32e mode; execute-disable needs `IA32_EFER.NXE=1`. https://cdrdv2.intel.com/v1/dl/getContent/671190 (Intel SDM Vol. 3A, order 253668-093US, 2026-09)
- `x86_64` 0.15.5 provides `OffsetPageTable`, `MappedPageTable`, `RecursivePageTable` and the `gdt`, `idt`, `paging`, `port`, `tss` modules. https://docs.rs/x86_64/0.15.5/x86_64/structures/paging/mapper/index.html
- LA57 exists in QEMU and on Zen 4 and Ice Lake class hosts, but the kernel does not need it.
- Reclaim prior art:
  - DAMOS: a scheme pairs an access pattern with an action, with quotas, goal-based tuning (including a `user_input` metric) and watermarks. https://raw.githubusercontent.com/torvalds/linux/master/Documentation/mm/damon/design.rst
  - MGLRU: PID-style refault feedback and a `min_ttl_ms` working-set guard. https://raw.githubusercontent.com/torvalds/linux/master/Documentation/mm/multigen_lru.rst and https://raw.githubusercontent.com/torvalds/linux/master/Documentation/admin-guide/mm/multigen_lru.rst
  - PageFlex delegates only prefetching and proactive reclaim to userspace via eBPF; slowdown under 1%. https://cseweb.ucsd.edu/~snoeren/papers/pageflex-atc25.pdf (USENIX ATC '25)
  - PSI reports "some" and "full" stall percentages; trigger windows 500 ms to 10 s. https://raw.githubusercontent.com/torvalds/linux/master/Documentation/accounting/psi.rst

### 5. Interrupts, SMP and timers

- IDT: 256 entries via `x86_64`; IST stacks for double fault, NMI and machine check. MADT gives LAPIC IDs, IOAPICs and interrupt source overrides (QEMU overrides the PIT's IRQ0 to GSI 2). Use x2APIC when CPUID.01H:ECX[21] is set. MSI-X for virtio-pci at M6.
- TCG emulates x2APIC since a series merged in February 2024 (likely QEMU 9.0). https://patchwork-proxy.ozlabs.org/project/qemu-devel/patch/20240111154404.5333-1-minhquangbui99@gmail.com
- The LAPIC timer has one-shot, periodic and TSC-deadline modes (CPUID.01H:ECX[24], MSR 6E0H); it runs at the bus or core-crystal clock and may stop in deep C-states unless CPUID.06H:EAX[2] is set. https://cdrdv2.intel.com/v1/dl/getContent/671190
- TCG's x86 CPUID lists `CPUID_EXT_PCID` and `CPUID_EXT_TSC_DEADLINE_TIMER` as missing (verified in source), and `TCG_APM_FEATURES` is 0 so no invariant TSC. https://gitlab.com/qemu-project/qemu/-/raw/master/target/i386/cpu.c (2026-10-05). The emulated LAPIC runs on `QEMU_CLOCK_VIRTUAL` with no TSC-deadline path. https://gitlab.com/qemu-project/qemu/-/raw/master/hw/intc/apic.c (2026-06-18). Older TCG warns "doesn't support requested feature: CPUID.01H:ECX.tsc-deadline". https://gitlab.com/qemu-project/qemu/-/issues/1534
- Invariant TSC is CPUID 0x80000007 EDX[8]. Prefer a median over repeated calibration samples because host vCPU preemption makes single samples noisy.
- SMP via Limine: the BSP allocates each AP's stack and per-CPU block and writes `extra_argument` and `goto_address`; each AP loads the GDT/IDT/TSS, sets `GS_BASE`, enables its LAPIC and enters the idle loop; then add IPIs. Without a bootloader the BSP needs INIT-SIPI-SIPI with 10 ms and 200 microsecond delays. https://cdrdv2.intel.com/v1/dl/getContent/671190
- VIRTIO 1.4 (2026-04-08): virtio-pci maps config-change and queue interrupts to MSI-X vectors via `config_msix_vector` and `queue_msix_vector`; with MSI-X enabled the ISR register is unused. https://docs.oasis-open.org/virtio/virtio/v1.4/virtio-v1.4.html

### 6. Scheduler prior art

- sched_ext (merged in Linux 6.12): a BPF program implements `sched_ext_ops` (select_cpu, enqueue, dequeue, dispatch, tick, running, stopping, set_weight, init_task/exit_task, init/exit). Each CPU runs from its local dispatch queue; a global FIFO is shared; custom queues are FIFO or virtual-time ordered. On error, a stalled runnable task or SysRq-S the default behaviour is restored and all tasks revert to the fair class. `SCX_OPS_SWITCH_PARTIAL` confines the policy to tasks that opt in. A bypass mode gives every task a fixed slice (`slice_bypass_us`, 5000 by default). The watchdog `timeout_ms` defaults to and cannot exceed 30 s. https://docs.kernel.org/scheduler/sched-ext.html, https://raw.githubusercontent.com/torvalds/linux/master/Documentation/scheduler/sched-ext.rst, https://raw.githubusercontent.com/torvalds/linux/master/kernel/sched/ext/internal.h and https://raw.githubusercontent.com/torvalds/linux/v6.12/kernel/sched/ext.c
- scx (GPL-2.0, active) includes `scx_rustland`, which decides in a user-space Rust process; `scx_rustland_core` defines events in (`QueuedTask`) and decisions out (`DispatchedTask` with `slice_ns`). https://github.com/sched-ext/scx and https://raw.githubusercontent.com/sched-ext/scx/main/rust/scx_rustland_core/assets/bpf.rs (crate 2.4.14, 2026-08-19)
- ghOSt (SOSP '21) runs policy in userspace agents with a watchdog and CFS fallback; costs: 265 ns message delivery, 888 ns local schedule, 5,688 ns for a group of 10 remote schedules. The userspace repo has been archived since 2025-02-17. https://storage.googleapis.com/gweb-research2023-media/pubtools/6413.pdf and https://github.com/google/ghost-userspace
- Theseus `Scheduler` trait (next, add, remove, drain, busyness) and `set_policy(cpu_id, scheduler)` that drains the old scheduler into the new one (MIT, last commit 2024-09-22). https://raw.githubusercontent.com/theseus-os/Theseus/theseus_main/kernel/task/src/scheduler.rs
- OSTD (design reference only): `Scheduler<T>` with `enqueue(Arc<T>, EnqueueFlags) -> Option<CpuId>`, `local_rq_with`, `mut_local_rq_with`; `LocalRunQueue<T>` with `current`, `update_current(UpdateFlags) -> bool`, `try_pick_next`, `dequeue_current`; `inject_scheduler` is callable once; preemption is documented only for user-mode tasks. It has no wakeup-placement split, watchdog or hints channel. https://asterinas.github.io/api-docs/0.19.0/ostd/task/scheduler/trait.Scheduler.html, https://asterinas.github.io/api-docs/0.19.0/ostd/task/scheduler/trait.LocalRunQueue.html, https://raw.githubusercontent.com/asterinas/asterinas/main/ostd/src/task/scheduler/mod.rs and https://docs.rs/ostd/0.18.1/ostd/task/scheduler/index.html (docs.rs failed to build 0.19.0: https://docs.rs/crate/ostd/0.19.0). Preemption is documented only for tasks in user mode: https://asterinas.github.io/api-docs/0.19.0/ostd/task/scheduler/fn.enable_preemption_on_cpu.html. `GlobalFrameAllocator` has `alloc(Layout)`, `dealloc`, `add_free_memory`. https://asterinas.github.io/api-docs/0.19.0/ostd/mm/frame/allocator/trait.GlobalFrameAllocator.html; the slab policy is `GlobalHeapAllocator`: https://asterinas.github.io/api-docs/0.19.0/ostd/mm/heap/trait.GlobalHeapAllocator.html. Scheduler, frame allocator and slab allocator are safe-Rust policies injected via traits. https://raw.githubusercontent.com/asterinas/asterinas/main/book/src/ostd/soundness/safe-policy-injection.md
- Control-plane designs: SchedCP keeps the LLM in a control plane and gates deployment through the eBPF verifier, static analysis, micro-VM tests and canaries (workshop paper). https://arxiv.org/pdf/2509.01245v4. AKTS verifies a policy library once and the agent writes only an integer index; a switch takes 920 ns (p50) (workshop paper). https://arxiv.org/abs/2609.12276. LumOS stages bounded CFS parameter sets, with `commit(token)` or `revert(token)`, rate limits and cooldowns. https://liargkovas.com/assets/pdf/Liargkovas_ML4Sys_NeurIPS25.pdf
- `rbpf`'s verifier is "very simple (nothing close to the one for Linux kernel)" and its JIT is unavailable under `no_std`. https://raw.githubusercontent.com/qmonnet/rbpf/main/README.md (0.4.1, 2026-02-06)
- Synthetic workloads: schbench, rt-app and stress-ng assume Linux or POSIX, so workload shapes (CPU hogs, periodic interactive tasks, request/response, IPC storms, working-set sweeps) must be reimplemented as native ring-3 programs (budget in M10). https://git.kernel.org/pub/scm/linux/kernel/git/mason/schbench.git/plain/README.md, https://git.kernel.org/pub/scm/linux/kernel/git/mason/schbench.git/plain/schbench.c, https://raw.githubusercontent.com/scheduler-tools/rt-app/master/README.in and https://raw.githubusercontent.com/ColinIanKing/stress-ng/master/README.md

### 7. Reference kernels

- **Redox** (MIT; https://github.com/redox-os/kernel, GitLab https://gitlab.redox-os.org/redox-os/kernel): Unix-like microkernel with user-space drivers and URL-like schemes; own libc (relibc) and Orbital GUI. Latest release 0.9.0 was released 2024-09-07 (GitLab release record; its `created_at` of 2025-09-22 is what the local memo mistook for the release date; the other local memo said 2024-09-09). https://gitlab.redox-os.org/api/v4/projects/redox-os%2Fredox/releases. 2026 activity: rustc and cargo self-hosted in January (https://phoronix.com/news/redox-os-january-2026), signals, capabilities and an EEVDF goal in February (https://redox-os.org/news/this-month-260228/), an NLnet virtualization grant in June (https://www.phoronix.com/news/Redox-OS-June-2026), io_uring-like API and NUMA in the August report (https://phoronix.com/news/Redox-OS-August-2026). Read its x86_64 startup, interrupt code and scheme IPC.
- **Theseus** (MIT; https://github.com/theseus-os/Theseus): single address space, single privilege level, cells linked at runtime. Dormant (last commit 2024-09-22, no releases). Closest prior art for live reload, but built on the model the founder rejected. The memo on live editing covers it.
- **Hubris** (MPL-2.0; https://github.com/oxidecomputer/hubris, release `all-sp-v1.82.0` 2026-10-07): Cortex-M OS with build-time tasks, no kernel allocation, synchronous IPC, MPU isolation and a supervisor that restarts faulted tasks. No x86 code to reuse; model for the restarting supervisor.
- **Maestro** (AGPL-3.0; https://github.com/maestro-os/maestro, last push 2026-10-10, last commit 2026-10-04): Linux-compatible-by-goal monolith, self-described "highly unstable". Read for orientation only.
- Other readable x86_64 Rust kernels with SMP or preemption: Hermit (MIT OR Apache-2.0), ArceOS (GPL-3.0-or-later OR Apache-2.0 OR MulanPSL-2.0), Aero (GPL-3.0). Read the copyleft ones for ideas only.
- **Asterinas** (https://github.com/asterinas/asterinas, MPL-2.0, x86-64 Tier 1): `ostd` and `cargo-osdk` 0.19.0 were published on crates.io 2026-10-12 and announced 2026-10-13 (https://crates.io/api/v1/crates/ostd, https://asterinas.github.io/2026/10/13/announcing-asterinas-0.19.0.html). Minor releases 0.16.0 (2025-08-05), 0.17.0 (2025-12-13), 0.18.0 (2026-06-03), 0.19.0. `UserMode::execute` changed signature between 0.17.2 and 0.18.1 (https://docs.rs/ostd/0.17.2/ostd/user/struct.UserMode.html, https://docs.rs/ostd/0.18.1/ostd/user/struct.UserMode.html). Boot is Multiboot, Multiboot2, Linux protocol and PVH, with no Limine (https://raw.githubusercontent.com/asterinas/asterinas/01acaa7eeed755a32fc658e7d732b080061b4872/ostd/src/arch/x86/boot/mod.rs). Kernel page tables and `KVirtArea` are `pub(crate)` and Asterinas will not load kernel modules (https://raw.githubusercontent.com/asterinas/asterinas/01acaa7eeed755a32fc658e7d732b080061b4872/ostd/src/mm/kspace/mod.rs, https://asterinas.github.io/book/kernel/linux-compatibility/index.html). Only OSTD may use `unsafe` (https://asterinas.github.io/book/kernel/the-framekernel-architecture.html).
- MPL-2.0: modified or new files containing covered code are Modifications; distributing binaries requires source for covered files; a Larger Work may use other terms; new files with no MPL code are not Modifications. https://www.mozilla.org/media/MPL/2.0/index.txt and https://www.mozilla.org/en-US/MPL/2.0/FAQ/. Asterinas chose MPL so proprietary code may link, requires no CLA and uses `SPDX-License-Identifier: MPL-2.0`. https://raw.githubusercontent.com/asterinas/asterinas/main/book/src/README.md and https://raw.githubusercontent.com/asterinas/asterinas/01acaa7eeed755a32fc658e7d732b080061b4872/.licenserc.yaml

### 8. PCI, virtio and display

- `virtio-drivers` 0.13.0 (MIT, last push 2026-10-09): `no_std` drivers for block, net, GPU, input, console, socket, sound, RNG, 9P over MMIO and PCI. PCI transport supports "Memory-mapped CAM only, e.g. aarch64 or PCIe ECAM". No packed virtqueues. We implement `unsafe trait Hal` (DMA allocation, address translation, share/unshare) and handle interrupts. Config access is pluggable through `ConfigurationAccess` (`MmioCam` built in). https://raw.githubusercontent.com/rcore-os/virtio-drivers/master/README.md, https://github.com/rcore-os/virtio-drivers and https://raw.githubusercontent.com/rcore-os/virtio-drivers/master/src/hal.rs
- On x86 QEMU, PCIe features exist only on q35. https://gitlab.com/qemu-project/qemu/-/raw/master/docs/pcie.txt
- OVMF's virtio-gpu GOP is `PixelBltOnly` (no linear framebuffer); `QemuVideoDxe` gives a linear GOP for `-vga std`, `bochs-display`, virtio-vga, QXL, VMware SVGA and Cirrus. https://raw.githubusercontent.com/tianocore/edk2/master/OvmfPkg/VirtioGpuDxe/Gop.c and https://raw.githubusercontent.com/tianocore/edk2/master/OvmfPkg/QemuVideoDxe/Driver.c. Taking over virtio-gpu needs a reset and explicit transfer/flush; QEMU's default backend is 2D only. https://gitlab.com/qemu-project/qemu/-/raw/master/docs/system/devices/virtio/virtio-gpu.rst
- Driver order to M6: GOP framebuffer, PS/2 keyboard, virtio-net plus smoltcp (QEMU `-netdev user` gives DHCP and outbound TCP). Virtio-console is optional (the UART suffices). Virtio-blk comes later. The network memo covers the stack in detail.

### 9. QEMU hosts and accelerators

- **WSL2 + KVM (daily).** `nestedVirtualization` defaults to true on Windows 11. https://learn.microsoft.com/en-us/windows/wsl/wsl-config (2026-09-16). WSL2's kernel config builds `CONFIG_KVM=m` (https://raw.githubusercontent.com/microsoft/WSL2-Linux-Kernel/linux-msft-wsl-6.18.y/arch/x86/configs/config-wsl); Microsoft says non-Microsoft virtualization on Hyper-V "isn't supported" (https://learn.microsoft.com/en-us/virtualization/hyper-v-on-windows/user-guide/nested-virtualization). Verified on this machine: `/dev/kvm` present (`root:kvm`, mode 660), CPU shows SVM, WSL 2.7.8.0, Windows 11 Home build 26200, RTX 4070 Ti. Add the user to the `kvm` group; QEMU is not yet installed (`apt install qemu-system-x86 ovmf`). Benefits: mature APIC, TSC-deadline, a gdbstub with hardware breakpoints, display through WSLg. Keep build output on the Linux filesystem. Nested VM exits cost more; measure. WSL networking: default NAT reaches Windows via the host IP, not localhost (https://learn.microsoft.com/en-us/windows/wsl/networking); here it fell back to "VirtioProxy". Ubuntu's archive has QEMU 8.2.2 (24.04) and 10.2.1 (26.04). https://api.launchpad.net/1.0/ubuntu/+archive/primary?ws.op=getPublishedSources&source_name=qemu&exact_match=true&status=Published&ws.size=30
- **TCG.** Deterministic and complete for gdb. Slowdown versus native is unmeasured: local estimates 10-50x, cloud estimates 5-20x. Timing is not cycle-accurate. Latest QEMU tag 11.1.2 (2026-09-28); 11.1.0 was released 2026-08-11. https://www.qemu.org/2026/08/11/qemu-11-1-0/ and https://gitlab.com/api/v4/projects/qemu-project%2Fqemu/repository/tags?per_page=40&order_by=updated
- **WHPX (optional).** Maintained ("Supported", Microsoft co-maintainer; about 60 x86 commits March to August 2026, including x2APIC). https://gitlab.com/qemu-project/qemu/-/raw/master/MAINTAINERS and https://gitlab.com/api/v4/projects/qemu-project%2Fqemu/repository/commits?path=target/i386/whpx&per_page=60. Documented limits: guests that use MMX/SSE/AVX for MMIO are unsupported, legacy VGA modes are slow, do not set `kernel-irqchip` manually; it rejects `kernel-irqchip=split`. https://gitlab.com/qemu-project/qemu/-/raw/master/docs/system/whpx.rst and https://www.qemu.org/docs/master/system/whpx.html. The split-irqchip rejection and the x2APIC emulation mode are in https://gitlab.com/qemu-project/qemu/-/raw/master/accel/whpx/whpx-common.c and https://gitlab.com/qemu-project/qemu/-/raw/master/target/i386/whpx/whpx-all.c. Open bugs: SMP boot hang on build 26200 with `-smp cores=2` (https://gitlab.com/qemu-project/qemu/-/issues/3178), "Unexpected VP exit code 4" in early boot (https://gitlab.com/qemu-project/qemu/-/work_items/4385 and https://gitlab.com/api/v4/projects/qemu-project%2Fqemu/issues/4385, opened 2026-08-31), virtio-net hang on sustained download (https://gitlab.com/api/v4/projects/qemu-project%2Fqemu/issues/4492, 2026-09-17), `system_reset` hangs the guest (https://gitlab.com/api/v4/projects/qemu-project%2Fqemu/issues/4410, 2026-09-03). Issue 2887 (OVMF startup error) closed 2025-03-29, resolution unverified. https://gitlab.com/api/v4/projects/qemu-project%2Fqemu/issues/2887
- **gdb under WHPX.** Breakpoints and stepping landed 2022-04-06 (https://qemu.googlesource.com/qemu/+/d7482ffe9756919531307330fd1c6dbec66e8c32), later broke, and commit 19b48084 (2026-02-26, first in 11.0.0) re-enabled them (https://gitlab.com/api/v4/projects/qemu-project%2Fqemu/repository/commits/19b48084f71). WHPX sets no gdbstub breakpoint hook, so hardware breakpoints and watchpoints are unavailable. https://gitlab.com/qemu-project/qemu/-/raw/master/gdbstub/system.c and https://gitlab.com/qemu-project/qemu/-/raw/master/accel/whpx/whpx-accel-ops.c. Whether memory breakpoints stop a WHPX guest is untested. TCG advertises unlimited hardware breakpoints and watchpoints. https://www.qemu.org/docs/master/system/gdb.html
- **icount and replay.** `-icount shift=N` runs one instruction per 2^N ns of virtual time; `sleep=off` jumps to the next timer deadline; it is system-emulation only, not cycle-accurate and incompatible with multi-threaded TCG. https://www.qemu.org/docs/master/system/invocation.html and https://www.qemu.org/docs/master/devel/tcg-icount.html. Without it the virtual clock is the host monotonic clock. https://gitlab.com/qemu-project/qemu/-/raw/master/system/cpu-timers.c. QEMU exits with "-icount is not allowed with hardware virtualization". https://gitlab.com/qemu-project/qemu/-/raw/master/system/vl.c. Multi-threaded TCG is the x86 default unless icount or replay is on. https://gitlab.com/qemu-project/qemu/-/raw/master/accel/tcg/tcg-all.c. Record/replay needs icount and its documented devices are `ide-hd` and `rtl8139`, not virtio. https://www.qemu.org/docs/master/system/replay.html

### 10. Test harness and CI

- `isa-debug-exit` calls shutdown with `(val << 1) | 1`; defaults `iobase` 0x501, `iosize` 2. https://gitlab.com/qemu-project/qemu/-/raw/master/hw/misc/debugexit.c. blog_os passes `-device isa-debug-exit,iobase=0xf4,iosize=0x04 -serial stdio -display none` with success code 33 and a 5-minute per-test timeout. https://raw.githubusercontent.com/phil-opp/blog_os/post-12/Cargo.toml and https://os.phil-opp.com/testing/
- Integration test per small kernel, so one crash kills one test. Alternatively a linker-section registry like OSTD's `ktest`.
- `-s` opens gdb on TCP 1234, `-S` holds the guest; single-stepping runs with IRQs and timers off by default; build with frame pointers for backtraces. https://www.qemu.org/docs/master/system/gdb.html
- Since 2024-04-02, 2-vCPU GitHub-hosted Linux runners can use KVM after a udev rule (`KERNEL=="kvm", GROUP="kvm", MODE="0666"`). https://github.blog/changelog/2024-04-02-github-actions-hardware-accelerated-android-virtualization-now-available/. Public-repo runners are free, 4 CPUs and 16 GB. https://docs.github.com/en/actions/reference/runners/github-hosted-runners. The Ubuntu 24.04 image (20261004) has Rust 1.99.0 and no QEMU; `ubuntu-latest` moves to 26.04 between 2026-10-19 and 2026-11-19. https://raw.githubusercontent.com/actions/runner-images/main/images/ubuntu/Ubuntu2404-Readme.md. Hermit's CI uses `/dev/kvm` on hosted runners. https://raw.githubusercontent.com/hermit-os/kernel/main/.github/workflows/ci.yml. `Swatinem/rust-cache@v2` must run after toolchain setup. https://raw.githubusercontent.com/Swatinem/rust-cache/master/README.md

### 11. Learning path

- blog_os second edition has 12 posts (Freestanding Binary 2018-02-10 to Async/Await 2020-03-27) covering VGA text, exceptions, double faults, 8259 PIC, paging, heap and cooperative async. https://os.phil-opp.com/. It promised APIC and preemption posts that never appeared. https://raw.githubusercontent.com/phil-opp/blog_os/main/blog/content/edition-2/posts/12-async-await/index.md
- Edition 3 is six draft posts on the `edition-3` branch (last commit 2025-05-09, UEFI via OVMF, `bootloader` 0.11); a `threads` branch is also unpublished. https://github.com/phil-opp/blog_os/commits/edition-3 and https://github.com/phil-opp/blog_os/discussions/1172
- Text is CC BY-NC 4.0; code blocks are also MIT or Apache-2.0. https://raw.githubusercontent.com/phil-opp/blog_os/main/blog/content/README.md
- VGA text mode does not exist under UEFI. The Testing, Double Faults and Paging posts transfer almost directly (the Paging post uses `OffsetPageTable`: https://raw.githubusercontent.com/phil-opp/blog_os/main/blog/content/edition-2/posts/09-paging-implementation/index.md).

### 12. Milestone ladder

Effort for one experienced Rust developer writing a first kernel. Local: full-time weeks of about 40 h (FT) and part-time weeks of about 15 h (PT). Cloud: person-weeks of 30-35 focused hours, plus or minus 50%. All are estimates.

| Milestone | Deliverable | Exit test | Local | Cloud |
|---|---|---|---|---|
| M0 Boot and harness | Pinned nightly, `x86_64-unknown-none`, Limine via OVMF and a virtual FAT directory, serial logger, GOP framebuffer fill, `xtask run/test`, CI, boot-to-prompt timing, guest-to-Mac TCP reachability check from WSL2 | `cargo xtask test` boots under KVM (WSL2) and TCG, logs to serial and exits via `isa-debug-exit`; CI green | 1 FT / 2-3 PT | 1-3 |
| M1 CPU and console | GDT/TSS with IST, IDT, exception handlers, panic backtrace (source-linked if the symbol index exists), bitmap-font framebuffer console | `int3` caught and resumed; page fault reports CR2; stack overflow hits the double-fault handler, not a triple fault | 1-2 FT / 3-4 PT | 2-4 with M2 |
| M2 Memory | Bitmap then buddy frames, `OffsetPageTable` over the HHDM, uncached MMIO mappings, guard pages, `talc` heap, `ReclaimPolicy` trait with CLOCK default | 1M-op alloc/free stress with no leak; map/unmap/protect tests; heap growth; host allocator tests pass | 2-3 FT / 4-6 PT | in M1 |
| M3 Interrupts and time | MADT/HPET/MCFG parsing, LAPIC, IOAPIC, PS/2 keyboard, calibrated one-shot LAPIC timer, monotonic clock | Keys echo; 1 kHz tick within 2% of the VM clock over 60 s; passes under KVM and TCG (WHPX best-effort) | 2-3 FT / 4-6 PT | 2-4 |
| M4 Preemptive scheduling (1 CPU) | Kernel threads, context switch, wait queues, timer preemption with preempt count, `SchedPolicy` trait, round-robin fallback, second policy, runtime swap, watchdog | 8 CPU hogs plus 1 interactive thread: key echo under 20 ms; a deliberately buggy policy is ejected and the system survives; simulator tests pass | 3-4 FT / 6-8 PT | 4-8 with M7 |
| M5 SMP | Limine MP bring-up, per-CPU data, per-CPU run queues, reschedule/TLB/panic IPIs, `select_cpu`, load balancing | `-smp 4`: all CPUs run tasks; 10-minute spawn/exit/sleep storm with no deadlock or lost task; TLB-shootdown test | 3-4 FT / 6-8 PT | 3-6 |
| M6 Network and terminal | PCI/ECAM, virtio-net plus smoltcp (DHCP, TCP), framebuffer terminal with line editing and scrollback, a kernel-resident fixed-command monitor (no Rhai yet) | In q35 `-smp 4`: a typed command sends HTTP to the Mac mini LLM endpoint and the reply renders; a policy swap from the prompt works | 3-5 FT / 6-10 PT | 3-6 (drivers only) |
| M7 User mode and supervisor | Per-process page tables, syscall/sysret with canonical-RCX check, ELF loader, SMEP/SMAP/UMIP, user heap, quotas, socket syscalls, ring-3 supervisor with restart | A ring-3 process that faults, spins forever or exhausts memory is killed and the supervisor restarts it; kernel keeps running | see memos 2, 3 | with M4 |
| M8, M9 | Rhai REPL, LLM scripts, Wasm live editing | See the compiler, LLM and live-edit memos | n/a | n/a |
| M10 AI policy demo | Policy table with bounded parameters, stage/commit/revert, stall watchdog, demand paging, accessed-bit scanning, clock reclaim, swap, ring-3 workload generators, deterministic icount runs, virtio-blk (new prerequisite) | Policy A/B ranking agrees between icount and KVM runs; LLM-selected policy beats the default on a synthetic workload; bad policy auto-reverts | n/a | 7-14 |

Founder-order changes the findings argue for: add virtio-blk (or an in-RAM swap) before M10; settle the kernel preemption model before M4; run the M3 and M5 exit tests under KVM and TCG only.

## Open questions

1. **Which QEMU version runs in WSL2, and does it include the features this memo cites from QEMU 11.x?** Ubuntu 24.04 ships 8.2.2. Decide at M0 whether to build 11.1.2 from source.
2. **Does QEMU slirp inside WSL2 (VirtioProxy networking) reach the Mac mini reliably?** Test guest TCP to the Mac's LAN address at M0. It also decides whether 10.0.2.2 assumptions in other memos hold.
3. **How large is nested-KVM overhead in WSL2** for boot time and virtio-net throughput? Measure at M0 and M6.
4. **Do WHPX bugs 3178 (SMP hang) and 4385 (early exit) reproduce** with QEMU 11.1.x on build 26200? Only matters if WHPX is used. Test a minimal Limine kernel at `-smp 4`, 20 runs.
5. **Is TSC-deadline exposed under KVM in WSL2?** Feature-detect either way; confirm in the M0 CPUID dump.
6. **How much slower is TCG than KVM on this machine?** Local guesses 10-50x, cloud 5-20x. Time a fixed CPU-bound loop in the guest under both.
7. **Which icount `shift` and `-smp` define the canonical measurement mode,** and is single-threaded TCG representative enough for SMP policy comparisons? Resolve at M10 by comparing a policy pair under icount and KVM.
8. **Do virtio-blk and virtio-net work under record/replay?** Docs show only `ide-hd` and `rtl8139`. Resolve when virtio-blk lands; fall back to plain icount.
9. **Which OVMF build to pin** once QEMU drops `edk2-i386-vars.fd`? Vendor it through `ovmf-prebuilt` and record the edk2 tag.
10. **Does KVM work on GitHub-hosted runners in 2026?** Confirmed only by a 2024 changelog and Hermit's CI. Verify on the first CI run; fall back to TCG automatically.
11. **Can the kernel move to stable Rust?** Not while `limine` 0.6.5 uses `ptr_metadata` and `acpi` 6.1.1 uses `allocator_api`. Watch both crates and re-test a stable build in CI.
12. **Kernel licence and contributor terms:** MPL-2.0 vs Apache-2.0 OR MIT, and CLA vs DCO. Only a CLA keeps commercial relicensing of outside contributions possible. Get counsel before the first outside contribution and any commercial release.
13. **Transport security to the LLM server:** plain HTTP on the LAN is assumed. See the LLM memo (TLS stays out of the kernel).
14. **Does an in-kernel command monitor at M6 conflict with "shell scripts run in ring 3"?** Default: no, because it accepts a fixed command table only and is removed or demoted once the ring-3 shell exists at M8.
