# ADR 0007: The driver model

Proposed, 2026-10-21. The founder allowed C code in the kernel build on 2026-10-22 (question 1). The rest waits for approval.

Design only. Nothing was built, run or measured for this ADR, and no model ran anywhere. Repo facts are `path:line` on main at ec5ecff unless a branch is named; open PRs were read with `git show` at #16 131aa77, #18 af7635d and #19 0717caa. Licences marked "checked" were read at the source on 2026-10-21 by this topic's research and judge; uACPI, ACPICA, `acpi`, `pci_types`, virtio-drivers, FreeBSD's AMD IOMMU and igc, OpenBSD rge and its microcode, and NVIDIA's COPYING were checked again for this ADR the same day. Shared names and owners follow the P0 decision sheet: `Shm` and handles are [ADR 0001](0001-capabilities.md)'s, action names and tiers are [ADR 0002](0002-agent-actions.md)'s, licence classes are [ADR 0009](0009-licence.md)'s.

## Context

The roadmap's P0 row asks for "the driver model: virtio first, then real hardware" (docs/roadmap.md:139). P7 needs "UEFI GOP display, NVMe, USB (xHCI) keyboard and mouse, the board's network chip and ACPI power; then audio and basic GPU modesetting" (docs/roadmap.md:146), and drivers are one of the two long poles (docs/roadmap.md:158). Interfaces are versioned from their first release (docs/roadmap.md:131).

What the code does today:

| Area | Today | Where |
|---|---|---|
| Drivers | virtio-net, read-only virtio-blk, read-only NVMe, PS/2, GOP console. All in ring 0, all polled except PS/2. NVMe masks every vector (INTMS all ones). | kernel/src/nvme/controller.rs:126-127 |
| PCI | Segment 0 only, one bus mapped (`let buses = 1`), no bridges. Matching is hard-coded arms in `probe` and `Found`. #18 and #19 each add another arm. | kernel/src/virtio/pci.rs:62-71, :78-109 |
| Interrupts | Seven fixed vectors. Any other vector above 31 is fatal. No MSI or MSI-X. xAPIC only: the kernel refuses x2APIC. At most 8 CPUs. | kernel/src/apic.rs:9-15, :62-63; kernel/src/idt.rs:122; kernel/src/percpu.rs:12 |
| DMA | No IOMMU code. No way to ask for memory below 4 GiB (only a lower bound). Running out of bounce frames panics. Device mappings are permanent. | mm/src/frames.rs:115-119; kernel/src/virtio/hal.rs:95-96; kernel/src/memory/mod.rs:36 |
| Ring 3 | No port access (default TSS, IOPL 0 on return). No device calls (calls 0-36). Nine capability bits, none for devices. Copy IPC of at most 8 KiB. 16 processes. MODEL_MAP is the one precedent for physical memory in ring 3. | kernel/src/gdt.rs:78; kernel/src/process/mod.rs:293-294, :54; kabi/src/call.rs:86-92; kabi/src/lib.rs:82-108, :160 |
| Restart | The supervisor restarts services with 50 ms to 1.6 s backoff and gives up after 30 restarts in 60 s. Kernel drivers have no restart path; panic is abort. | supervise/src/lib.rs:158-164; docs/decisions.md:54; kernel/src/main.rs:453-460 |
| ACPI | Tables only (MADT, HPET, MCFG), "No AML". No power-off or reboot. The i8042 drain at boot has no bound. | acpitables/src/lib.rs:1-4; kernel/src/i8042.rs:92-95 |
| Boot order | The kernel loads the model from NVMe or virtio-blk before user space starts. | kernel/src/main.rs:245-249 |

PCIDs are off, so every switch into a ring-3 driver flushes its TLB entries (kernel/src/memory/user.rs:9). A server waits at most 60 s in one `svc_recv` (kernel/src/ipc.rs:29); a caller may wait up to 300 s (kernel/src/process/syscall.rs:21). The polled, one-command-at-a-time NVMe driver loads the 2.5 GB Qwen3-4B file (xtask/models.json:156) in 5 to 10 s (docs/decisions.md:211, measured under VirtualBox on the founder's PC on 2026-10-20).

Open work that touches drivers: #19 adds the `xhci` crate (3,008 lines in xhci/src plus a 179-line HID test file, 35 tests, counted at 0717caa). It reaches hardware only through a `Hal` trait "so the same driver can run in the kernel or in a user-space driver process" (xhci/src/lib.rs:5-8 on the branch), and it polls from the timer interrupt on CPU 0 (kernel/src/usb.rs:148-175 and kernel/src/idt.rs:98-102 on the branch). #18 adds virtio-gpu 2D behind a back buffer and `flush(rect)`, with GOP as the fallback (kernel/src/display/mod.rs:1-12 on the branch). #16 adds RESOLVE as call 37 (kabi/src/call.rs:134 on the branch) and a `net` thread that polls the card.

The founder's PC is a Ryzen 7 7700X (16 threads) with an RTX 4070 Ti on an AM5 board whose model is unknown. Its NIC (likely RTL8125 or I226-V), audio (USB ALC4080 or HD Audio) and firmware settings are unverified.

## Options considered

Three designs were judged. All three keep driver logic in host-tested `no_std` crates.

| Design | Score | What it proposed | Why it won or lost |
|---|---|---|---|
| A, user-space drivers | 6 | Every DMA-capable device in ring 3 (NVMe, virtio-blk, virtio-net, virtio-gpu, xHCI), nine device calls, versioned shared-memory rings, IOVAs from day one, a quiesce-before-free rule. About 9,000 lines in 12 PRs, before P2. | Lost. Strongest isolation and the cleanest end state, but the heaviest cost before v0.1. It moves the model disk into ring 3, which changes the boot order (main.rs:245-249), and throws away the in-flight kernel glue within months. Its data path is unmeasured, and without AMD-Vi the isolation is crash-only. Its ring format, IOVA naming and quiesce rule are kept. |
| B, kernel drivers with host-tested logic | 6 | Everything in ring 0 through P7, a shared `devhal`, a `Driver { poll, reset, health }` lifecycle, a device status record, watchdog resets, and panic containment on driver threads. | Lost. Fastest to v0.1, but device-supplied USB descriptors are parsed in ring 0 and drivers cannot be updated without a reboot, against decisions.md:14. Its core recovery idea, containing a panic on a driver thread, has no mechanism here: panic is abort, there is no unwinding, and held locks and heap would leak (decisions.md:54; main.rs:453-460). Its lifecycle, status record, "no driver work in interrupt context" rule and test plan are kept. |
| C, reuse-first hybrid | 8 | One driver contract with a ring-0 and a ring-3 host, so placement is a table row chosen by risk. Boot-critical and hypervisor devices stay in ring 0; pluggable buses, foreign ports and C go to ring 3. Reuse of uACPI, FreeBSD's AMD IOMMU, OpenBSD rge and FreeBSD igc. About 4,500-6,000 lines over P1-P3. | Won, with grafts from A and B and one change: AML stays in the kernel. C ran uACPI in a ring-3 `acpid`, which needs wide port and memory access and couples poorly to the SCI, sleep and `_OSC`; its user-space prior art rests on uACPI's README only (unverified). |

Points the judge corrected, so they are not used as arguments here: B's figure of about 305,000 copies to load the model through ring 3 assumes copy IPC, which A and C do not use. QEMU's emulated AMD IOMMU translating virtio and qemu-xhci DMA under KVM is unverified (spike S07-2). xAPIC's 8-bit IDs covering the 7700X rests on firmware handing over in xAPIC mode, unverified for the founder's board.

Reuse options, with licences:

| Source | Licence | Use | Verdict |
|---|---|---|---|
| uACPI 7.0.0 (2026-10-16) | MIT, checked | AML interpreter, GPEs, S-states, `_OSC`, `_PRT` | **Adopted**, in the kernel, gated by S07-3. About 35,000 lines of C (design C's count, not re-checked). |
| ACPICA 20260930 | Intel, or BSD-3-Clause, or GPL-2.0-only | Same job, heavier OS layer | **Fallback** under BSD-3-Clause if S07-3 fails. |
| `acpi` 6.1.1 AML | MIT/Apache-2.0, checked | Rust AML | **Rejected**: Redox, its main real-hardware user (through its own fork of the 6.x branch), lists "The AML parser does not work on real hardware" in its open driver issue 56. |
| `pci_types` 0.10.1 | MIT/Apache-2.0, checked | Headers, BARs, bridges, MSI and MSI-X capabilities | **Adopted** (new direct crate, free class). |
| virtio-drivers 0.13.0 | MIT, checked | virtio devices | **Kept**; its `device::sound` serves `audiod` in P6. |
| FreeBSD `sys/x86/iommu/amd_*.c` | BSD-2-Clause, checked | AMD-Vi device table, command and event rings, page tables, interrupt remapping | **Ported** to a Rust `amdvi` crate against AMD doc 48882 rev 3.10. No Rust OS the research checked has AMD-Vi (Asterinas and Theseus are VT-d only). |
| OpenBSD `if_rge.c` rev 1.47 | ISC, checked | RTL8125 bring-up | **Port at P7.** Its microcode `rgemcode.h` rev 1.1 is Realtek's: the notice grants distribution only, with no permission to modify, so it is not open source (checked). |
| FreeBSD `sys/dev/igc` | BSD-2-Clause (`if_igc.c`) and BSD-3-Clause (Intel's shared code, `igc_api.c`), checked | I226-V | **Port at P7** if the board has it. |
| Redox `pcid`, `xhcid`, `usbhidd`, `ihdad` | MIT | MSI-X, USB quirks, HD Audio | **References** and ports of logic; the crates need `std` and schemes. |
| Managarm drivers | MIT | rtl8168 (names RTL8125), igc, nvidia-open | **References** only (C++ on its own IPC). |
| NVIDIA open-gpu-kernel-modules | Files MIT, checked; GSP firmware under NVIDIA's licence | RTX 4070 Ti beyond GOP | **After v1.0**, ring 3, as Managarm runs it. |
| seL4 sDDF, Hubris, Fuchsia DFv2 | BSD-2-Clause; MPL-2.0; BSD-3-Clause style | Queue design; notification model; restart policy | **Ideas only.** |
| Asterinas VT-d and NVMe | MPL-2.0 | Intel IOMMU later | **Not used.** MPL is ask class since 2026-10-21; not needed before v1.0. |
| NetBSD rump kernels | BSD | Whole NetBSD drivers | **Option, not plan**: rumprun last moved 2020-05-11, buildrump.sh 2018-05-05. |
| Linux drivers, nova-core, Realtek r8125, Genode DDE-Linux | GPL-2.0; GPL-2.0; GPL-2 (unverified at source); AGPLv3 with GPL code | Everything | **Excluded.** A porter does not read them for the driver being ported. |

## Decision

The drivers that boot the machine and load the model (the disks, the VM's network card and the screen) stay inside the kernel, where they are today, so v0.1 is not held up. Drivers that talk to plug-in or untrusted hardware (USB, the real PC's network chip and audio) run as separate programs, so a crash restarts that driver, not the computer. Every driver is written once against one interface, so moving it into or out of the kernel later is a small change, not a rewrite. Power-off and the power button come from uACPI, a widely used open-source ACPI library in C, if you approve C code in the kernel (question 1).

In detail: base design C. Grafts: B's driver lifecycle, status record and tests; A's ring format, IOVA naming and quiesce rule. AML stays in the kernel.

### 1. Placement

A driver runs in ring 0 if it is needed before the supervisor starts or on the model or root-disk path, its device is emulated by the hypervisor or soldered to the board, and its code is ours or a small Rust crate we have read. Otherwise it runs in ring 3. Mechanism and platform code are always ring 0, whoever wrote them: interrupt controllers, timers, PCI config, the IOMMU and ACPI, AML included. The mechanism part is never hot-swapped (docs/roadmap.md:47).

| Device | Where | When | Gate |
|---|---|---|---|
| APICs, HPET or TSC, PCI config, the vector table, the IOMMU | ring 0 | now | mechanism |
| ACPI tables and AML (uACPI) | ring 0 | P1 tables, P3 AML | S07-3 for AML |
| Hardware watchdog: i6300esb (QEMU), the AMD TCO timer (P7; unverified on the founder's board) | ring 0 | P2 for QEMU, P7 for the board | mechanism; ADR [0008](0008-updates.md)'s fault path uses it |
| NVMe, virtio-blk, virtio-net, virtio-gpu, GOP, PS/2 | ring 0 | now | boot path or hypervisor device |
| xHCI and USB HID, later hubs | ring 3, `usbd` | P2, in v0.1 | S07-1; on a fail it stays in ring 0 on MSI-X |
| Audio: virtio-snd (P6), HD Audio or USB Audio Class (P7) | ring 3, `audiod` | P6, v0.2 | S07-1's teardown result |
| The P7 NIC (RTL8125 or I226-V) | ring 3, `netd` | P7, v0.3 | S07-4; on a fail it stays in ring 0 behind the same contract |
| RTX 4070 Ti beyond GOP | ring 3 | after v1.0 | founder question 2 |

v0.1 gets everything above except `amdvi`, `netd`, `audiod` and the board's TCO watchdog.

### 2. One contract, two hosts: `devkit`

A new `no_std`, host-tested crate, grown from #19's `xhci::Hal`. Driver crates depend on it and never import the kernel.

```rust
pub enum Zone { Any, Below4G }
pub trait Host {
    fn dma(&mut self, pages: usize, zone: Zone) -> Result<Dma, DmaError>; // contiguous, zeroed; never panics
    fn mmio(&mut self, bar: u8, offset: u64, len: u64) -> Result<Mmio, DmaError>; // bounds-checked, uncached
    fn irqs(&mut self, wanted: u16) -> u16;              // MSI-X, else MSI, else 0 = polled
    fn wait(&mut self, mask: u64, timeout_ns: u64) -> u64; // fired bits; unmasks the vectors waited on
    fn cfg_read(&self, offset: u16) -> u32;
    fn cfg_write(&mut self, offset: u16, value: u32);      // own function only, filtered
    fn now_ns(&self) -> u64;
}
pub trait Driver {
    fn poll(&mut self, budget: u32) -> Progress;  // at most `budget` completions
    fn reset(&mut self) -> Result<(), DriverError>;
    fn health(&self) -> Health;                    // Ok | Stalled | Fatal(code), from status registers
}
```

- `Dma::device()` names the address a device uses. It is an IOVA under an IOMMU and the physical address without one. Driver crates call it `device`, not `phys`, from the devkit PR on.
- Two hosts: `KernelHost` (kernel/src/devhost.rs: frames from `memory`, `map_physical`, the vector table, HPET or TSC) and `UserHost` (a new `user/devlib` over the calls in section 7). Moving a driver between rings swaps its host and its table row; the crate does not change.
- Class interfaces sit beside it: `Block` (ADR [0004](0004-storage.md)'s `block::Device` with write, flush and info), `Net` (frames in and out, under smoltcp's `Device`), `Input` (typed events, section 9), `Display` (#18's back buffer plus `flush(rect)`) and `Power` (off, reboot, button).
- `xhci::Hal` stays as #19 has it. One adapter of about 60 lines implements it over `devkit::Host`, in the kernel today and in `usbd` later.

### 3. PCI, `devmatch` and the device record

- A kernel `pci` module (kernel/src/pci/) on `pci_types` replaces the enumeration inside `virtio`. It walks every MCFG segment and bus range, maps each bus (1 MiB of ECAM) only when the walk reaches it, follows bridges through the secondary bus numbers firmware set, and leaves BARs as firmware assigned them. No BAR reassignment and no hot-plug before v1.0. Capability walks stop after 48 hops, as `split::common_cfg` does today (kernel/src/virtio/split.rs:38).
- `devmatch`, a `no_std` host-tested crate, holds the compiled-in table that replaces the arms in `probe` and `Found`. The kernel stays a static binary (docs/decisions.md:14).

```rust
pub enum Key { Class(u8, u8, u8), Id(u16, u16), Virtio(u16) }   // Id rows beat Class rows
pub enum Place { Kernel, User { service: &'static str, program: &'static str } }
pub struct Row { key: Key, driver: &'static str, place: Place, essential: bool, input: bool }
```

- `drv=usbd:kernel,netd:off` on the kernel command line moves or disables a driver for bring-up on new hardware.
- `essential` means the supervisor never gives up on that service; it keeps restarting at the longest backoff, 1.6 s. The plan line carries it, since restart policy is per service (docs/decisions.md:70). `usbd` is essential when the FADT says no 8042 exists.
- Each bound device has a versioned 64-byte `DeviceRecord` in `kabi`:

```rust
#[repr(C)] pub struct DeviceRecord {        // version 1, 64 bytes
    version: u16, index: u16, segment: u16, bus: u8, devfn: u8,
    vendor: u16, device: u16, class: [u8; 3], place: u8,      // Kernel | User
    state: u8, irq_mode: u8, iommu: u8, _r: u8,               // Bound | Running | Resetting | Restarting | Failed | Off; Polled | Msi | MsiX; confined or not
    driver: [u8; 16], resets: u32, interrupts: u64, dma_pages: u32, last_error: u32, _pad: [u8; 8],
}
```

It feeds DEV_LIST and P1's `device_list` action. A driver whose device the IOMMU does not confine says so in `iommu`, and the agent can tell the user.

### 4. Interrupts

- `kernel/src/irq.rs` allocates vectors 0x30-0xEF. The fixed vectors (kernel/src/apic.rs:9-15) do not move. In `trap_dispatch` the last arm (kernel/src/idt.rs:122) looks in the table first. A vector in 0x30-0xEF that nobody owns is counted and dropped, not fatal.
- Each function uses MSI-X, else MSI, else polling. INTx is never used: it needs AML `_PRT` (docs/research/rust-kernel-foundation.md:65). HD Audio controllers may offer MSI only (unverified for the founder's board); "else MSI" covers them.
- A handler only masks, acknowledges and wakes. Driver work never runs in interrupt context. Kernel drivers run on their own kernel thread; ring-3 drivers are woken through a notification bit (section 7), Hubris-style.
- Network and block drivers drain at most 64 completions per wake, then unmask (Linux NAPI).
- xAPIC stays until P7 (docs/decisions.md:57). P7 adds x2APIC and more than 8 CPUs.

### 5. DMA and the IOMMU

- `mm` gains an upper-bounded search for `Zone::Below4G`, which ends #19's below-4 GiB leak (kernel/src/usb.rs:36-38 on the branch). Every DMA allocation returns an error instead of panicking, which removes the bounce `expect` (kernel/src/virtio/hal.rs:96).
- Each claim keeps a DMA ledger of every frame given to its device.
- `amdvi`, a host-tested crate ported from FreeBSD `sys/x86/iommu/amd_*.c` against AMD doc 48882 rev 3.10, with IVRS parsing added to `acpitables`. One domain per ring-3 claim. Kernel drivers get pass-through until v1.0. Interrupt remapping is in its scope; until it lands, a device can still raise any vector, which is why stray vectors are dropped, not fatal.
- `iommu=prefer` (default: confine when IVRS exists, else run unconfined and say so in the record) or `iommu=require` (a ring-3 driver whose device is not covered is not started). The founder's PC runs `require` from v0.3.
- A virtio device obeys the IOMMU only with `VIRTIO_F_ACCESS_PLATFORM` (bit 33), which virtio-drivers 0.13 never accepts (no device's `SUPPORTED_FEATURES` includes it, for example src/device/net/mod.rs:201-205 in the 0.13.0 crate). `SplitTransport` overrides `write_driver_features` (kernel/src/virtio/split.rs:95) and can add it, but only virtio-net uses it today; virtio-blk still uses the plain `PciTransport` (kernel/src/virtio/pci.rs:145) and must move first, and the virtio HAL's `share` must then return IOVAs. This matters for S07-2 and for any virtio device a ring-3 driver owns; kernel drivers stay on pass-through.
- Intel VT-d waits for broader hardware after v1.0.

### 6. Kernel drivers: lifecycle and faults

- Panic stays abort (docs/decisions.md:54). B's panic containment is rejected.
- Each kernel driver runs on its own kernel thread. A software watchdog (not the hardware one in section 1) calls `reset()` on a command timeout (NVMe 5 s, kernel/src/nvme/controller.rs:19), on a fatal status (NVMe CSTS.CFS, xHCI USBSTS.HCE or HSE, virtio DEVICE_NEEDS_RESET) or after 2 s with work queued and no progress. If `reset()` fails, the device is marked `Failed` and its clients get errors.
- Every reset is logged as a system event the agent can read, in ADR 0002's `actlog 1`.

### 7. Ring-3 drivers: the device ABI

- **Authority.** A `Device` handle (ADR 0001 kind) per matched device, given at spawn only to the service a `Place::User` row names, running that row's program. This is how `cap::MODEL` is granted today (kabi/src/lib.rs:101-103). The kind's ceiling bit is the next free bit when the PR lands. Apps never hold `Device` handles; they reach devices through services (ADR [0003](0003-app-model.md)).
- **Calls**, named here and numbered in claim order from 38 when the PR lands (docs/decisions.md:123; RESOLVE is 37). Errors are today's `kabi::Error` values (kabi/src/lib.rs:25-45) plus ADR 0001's new ones.

| Call | Arguments -> result | Rule |
|---|---|---|
| DEV_LIST | buf, cap -> len | DeviceRecords. A `Device` handle shows its own; a caller with the INSPECT handle ADR 0001 requires for METRICS sees all. |
| DEV_CLAIM | device -> 0 | `Busy` while another claim lives or the last teardown is unfinished. Bumps the claim generation. |
| DEV_MAP | device, bar, offset, len, cache -> addr | Inside the BAR, in a new `layout::DEVICE_BASE` window. The MSI-X table page is refused, as in VFIO. Uncached or write-combined, never executable. |
| DEV_DMA | device, pages, zone, &mut DmaOut -> 0 | Counts against the page quota and the ledger. Returns the CPU address and the device address. Mapped read-write, never executable. |
| DEV_IRQ | device, wanted -> (granted, first bit) | The kernel programs MSI-X or MSI and binds each vector to a notification bit. |
| DEV_CFG | device, offset, width, value, write -> value | Own function. Writes to BARs, MSI and MSI-X capabilities, and command bits other than Memory Space and Bus Master are refused. |
| INPUT_PUSH | &[InputEvent], n -> n | Needs the key-sink handle ADR 0001 names, given at spawn only to the service whose `devmatch` row has `input`. |
| NOTIFY_WAIT | mask, timeout_ms -> fired | Unmasks the interrupt bits in `mask`, then sleeps until any bit in `mask` is set. Bit 63 means an IPC call is queued. |
| RING_ATTACH | shm, bit -> 0 | Doorbells on this ring set `bit` for the caller. |
| RING_NOTIFY | shm -> 0 | Sets the bit of every other process attached to the ring. |

- **Not built:** DEV_PORTS (its one user in design C was a ring-3 `acpid`; ring 3 keeps IOPL 0 and no I/O bitmap) and DEV_DMA_FREE (user pages are never unmapped while a process lives, kernel/src/memory/user.rs:10-11; drivers keep pools until user unmapping with TLB shootdown exists, which ADR [0001](0001-capabilities.md)'s `Shm` work adds in P1 (step 6); DEV_DMA_FREE may land with or after it).
- NOTIFY_WAIT and the two RING calls serve every KRNG user, not only drivers. If ADR 0001 adds a general wait on handles with the WAIT right, NOTIFY_WAIT becomes that call.
- **KRNG v1**, the one ring format for bulk data, over one `Shm` object (ADR 0001). Users: ring-3 drivers, ADR [0005](0005-graphics.md)'s `kai-surface 1` transport, ADR 0006's `vmm` exit ring, and ADR 0004's file service if its spike S04-2 moves bulk data off copied IPC.
  - Layout: a 64-byte header at offset 0; then `entries` 64-byte requests; then `entries` 16-byte responses; then a data area.
  - Header: `magic "KRNG"`, `version: u16 = 1`, `kind: u16`, `entries: u32` (a power of two, 8 to 4,096), `req_head`, `req_tail`, `rsp_head`, `rsp_tail: u32` (free-running), `want_req`, `want_rsp: u32` (a side sets these to ask for doorbells, as virtio's event suppression does); the rest is zero.
  - Request: `tag: u64, op: u16, flags: u16, buffer: u32, offset: u64, len: u32, _r: u32, args: [u64; 4]`. `buffer` 0 is the ring's own data area; 1 to 15 name further `Shm` objects passed with the ring, so bulk data never crosses IPC.
  - Response: `tag: u64, status: i32` (0 or a negative `kabi::Error`), `len: u32`.
  - One producer and one consumer per direction. A producer writes an entry, then publishes its head with release ordering; the consumer reads the head with acquire ordering.
  - `kind` numbers live in `kabi::ring`, claimed in order: BLOCK (ops READ, WRITE, FLUSH; semantics ADR 0004's, which needs no FUA), FRAME (net), MSG (ADR 0005's messages, at most 4,096 bytes in the data area), PCM (audio), VMEXIT (ADR 0006's exits and doorbells; read data is returned in the data area). A peer refuses any other version.
- **Teardown.** When a driver process exits, for any reason, the kernel does this before the supervisor sees the exit record: masks its vectors; clears Bus Master and Memory Space and reads the command register back; issues a Function Level Reset when the device offers one; drops the IOMMU domain after `COMPLETION_WAIT`; and only then frees the ledger. No frame is freed while a device can still reach it.
- **Restart.** Calls the driver had taken fail at once (kernel/src/ipc.rs:13-14). Ring requests in flight complete with `Dead` when the client re-attaches, and block clients resend. The supervisor restarts with today's policy, and the new instance resets the controller from scratch (#19's `Controller::start` already does, xhci/src/driver/mod.rs:132 on the branch).
- `usbd` takes one of today's 16 process slots (kernel/src/process/mod.rs:54). ADR [0003](0003-app-model.md) owns that limit and raises it to 64 in the first PR that needs more.

### 8. ACPI and power

- `acpitables` gains the FADT (reset register, PM1 blocks, SCI, and the `IAPC_BOOT_ARCH` 8042 flag) and IVRS. When the flag says no 8042 exists, `i8042::init` is skipped, and its drain gets a bound in any case.
- Reboot: the FADT reset register, with no AML; else the 8042 reset line when the FADT says one exists. Lands in P1.
- AML: uACPI 7.0.0 in the kernel, as platform code, imported whole into `third_party/uacpi/` with an `UPSTREAM.toml` (ADR 0009's import record), built by xtask with clang. It gives `\_S5` power-off, the power button through the SCI, and `_OSC` for native PCIe control. Lands in P3 so "turn off the computer" works in v0.1. Sleep (S3) is not promised before P7.
- The glue implements uACPI's kernel hooks: physical mapping, port I/O (kernel/src/port.rs has only `inb`, `outb` and `outl`, so it gains 16- and 32-bit reads and 16-bit writes), PCI config through `pci`, locks, an `acpi` kernel thread for work items, the SCI through the vector allocator, and the HPET clock. uACPI asks to map and unmap physical ranges, while `map_physical`'s window only grows (kernel/src/memory/mod.rs:36), so the glue keeps a bounded mapping cache. How often it maps is unverified until S07-3.
- If uACPI fails to start, reboot still works and power-off shows a "safe to switch off" screen. Once ADR [0008](0008-updates.md)'s `kernel/src/efi.rs` exists, UEFI `ResetSystem` is a further fallback for both.

### 9. Display, GPU and input

- virtio-gpu 2D in the VM and UEFI GOP on real hardware, both in ring 0, through v1.0. #18's "back buffer plus `flush(rect)`" is the `Display` contract. The kernel console stays the boot and panic screen (ADR 0005), so a ring-3 driver restart never takes it away.
- The RTX 4070 Ti gets GOP only through v1.0: one fixed mode.
- `InputEvent` is 16 bytes: `time_ns: u64, kind: u8 (Key, Rel, Abs, Button, Wheel), _r: u8, code: u16 (HID usage), value: i32`. PS/2 and `usbd` feed one kernel queue. Until P3's typed input lands, the kernel turns keys into scancode set 1, as #19's `i8042::inject` does.

### 10. Reuse, provenance and the licence gate

- Every ported file carries ADR 0009's import header: upstream SPDX id and copyright, our line, and `Upstream: <url> @ <commit> <path>`; the commit carries an `Imported-from:` trailer. `cargo xtask licences check` (ADR 0009) fails CI on any id outside the free class with no approving row; GPL, LGPL and AGPL driver code gets no row (question 6).
- New crate in the free class, listed here as the 2026-10-21 rule asks: `pci_types` 0.10.1 (MIT/Apache-2.0).
- Ask class, in the founder's questions: uACPI as C in the kernel and clang as a build tool; ported files under BSD-2-Clause, BSD-3-Clause, ISC and MIT, because ADR 0009's free class covers crates and its question 4 asks whether it covers copied files; firmware blobs. Nothing MPL-2.0 is used (Asterinas and Hubris are not ported).

### 11. Agent actions

P1 registers these rows in `agentabi` (ADR 0002). Nobody writes a tier by hand: it is derived by ADR 0002's table from each row's facts and inverse, and `min_tier` may only raise it.

| Action | Facts and inverse | Tier under ADR 0002 |
|---|---|---|
| `device_list` | read-only, closed-world | Run |
| `device_restart` | no inverse; idempotent (ring 0 calls `reset()`, ring 3 kills and lets the supervisor restart) | Ask. The judge proposed run; that needs ADR 0002 to count a restart that keeps no state as undoable. |
| `device_disable` | inverse `device_enable` | Ask, through `min_tier`: it can take away the keyboard |
| `device_enable` | inverse `device_disable` | Run |
| `power_reboot`, `power_off` | no inverse | Ask |

### 12. Testing

- Logic in host-tested crates: `devkit` bounds, `devmatch`, the DMA ledger, the MSI-X writer, DEV_CFG's filter, KRNG indices, `amdvi` page tables, `pci` walks over a fake ECAM.
- Hostile tests with `testkit/hostile.rs` (#13) and a nightly `cargo fuzz` job on the same targets.
- A fake-device harness: a `devkit::Host` that replays a recorded QEMU session with mutated registers and DMA contents, with a step budget so a hang is caught.
- New QEMU cases under KVM and TCG in CI: `pci-bridge` (virtio-net behind a `pcie-root-port`), `msix`, `usbd-restart`, `iommu`, `acpi-off` (QMP `system_powerdown`) and `dev-abi` (QEMU's `edu` device).
- VirtualBox and the founder's PC stay hand checks, with no model loaded for driver checks.

## Consequences

- v0.1 does not wait on a device ABI for its disks, network or display. Only `usbd` uses the ring-3 path in v0.1, and only after S07-1 passes.
- The model is still read by a kernel driver: before user space today (main.rs:245-249), and through ADR 0004's `MODEL_FILL` once it lives in kaifs. Neither path waits on a ring-3 driver.
- Until v1.0, a crash in the disk, VM network or display driver, or in uACPI, stops the whole machine. Watchdog resets cover hangs and fatal device states, not wild writes. USB, the P7 NIC and audio restart on their own.
- Device-supplied USB descriptors and HID reports are parsed in ring 3 once `usbd` lands; until then in ring 0 under hostile tests.
- Ring-3 drivers are crash-isolated at once and DMA-confined only after `amdvi` (P7), and only on AMD CPUs with the firmware IOMMU on.
- About 35,000 lines of C (unverified count) run in ring 0, interpreting firmware bytecode, and the build needs clang.
- Two hosts per contract means CI must run each driver in the ring it ships in.
- The device calls are private to in-tree programs (ADR 0001) and versioned with `kabi::ABI`; no third-party drivers before v1.0.
- KRNG v1 becomes a shared format for four ADRs, so changing it later means a version 2 that every peer understands.
- A ring-3 driver updates by restart; a kernel driver needs ADR [0008](0008-updates.md)'s A/B reboot.
- Provenance headers and firmware notices are a standing chore (ADR 0009).

## What it costs to change later

| Choice | Cost to reverse |
|---|---|
| Placement of one driver | Days: swap its host and its `devmatch` row; the crate does not change. |
| Device calls | Cheap until v1.0: only in-tree programs call them. Expensive once others write drivers. |
| KRNG v1 | Moderate: four users. The header's version field makes a v2 possible side by side. |
| uACPI | Contained: about 1,500 lines of glue to rewrite for ACPICA (BSD-3-Clause). |
| `devmatch` compiled in | Cheap. A loadable table later needs ADR 0008's update design. |
| No INTx | Cheap: uACPI can evaluate `_PRT` if a device ever lacks MSI. |
| Pass-through for kernel drivers | Moderate: giving them IOMMU domains touches every `KernelHost` DMA path. |
| Each foreign port | Grows with time: upstream fixes are merged by hand; the import header says from where. |
| Excluding Linux drivers | Expensive: reversing it puts GPL code in the image and needs the founder's licence decision. |
| Panic as abort for kernel drivers | The route out is moving a driver to ring 3, not unwinding in the kernel. |

## Spikes that settle it

All four run on GitHub Actions ubuntu-24.04 with apt QEMU 8.2 (.github/workflows/ci.yml:48-59; docs/decisions.md:214). Timings are judged under KVM; TCG checks function only. Local TCG in WSL2 (QEMU 10.2.1) is for function only and may differ from CI's 8.2. No model is loaded and no inference runs anywhere. Kernel AI does not boot on the founder's PC for any spike; S07-3's one host check there is that `cargo xtask build` finds clang.

**S07-1, a ring-3 interactive driver** (gates `usbd`, the device ABI and `audiod`; before P2's `usbd` step).
- Build: a throwaway branch with minimal MSI-X, DEV_CLAIM, DEV_MAP, DEV_DMA, DEV_IRQ, NOTIFY_WAIT, INPUT_PUSH, the exit teardown, and #19's `xhci` crate over `UserHost`. QEMU with qemu-xhci, usb-kbd and usb-tablet; 2,000 keys injected over QMP; three runs per accelerator.
- Pass:
  - interrupt to INPUT_PUSH: p99 at most 2 ms under KVM (10 ms under TCG), and at most 1 ms above #19's in-kernel path as merged, polled from the 1 kHz tick;
  - MSI-X to NOTIFY_WAIT return: p99 at most 50 µs idle, at most 200 µs beside 8 `hog` processes (user/src/bin/hog.rs);
  - `usbd` idle: at most 0.1% of one CPU over 30 s;
  - 100 random kills: no kernel fault; p99 from kill to the next key at most 500 ms; free frames back to baseline; at most 8 keys lost per kill; the "Bus Master off before free" assertion never fires.
- Also record the raw system-call round trip, for ADR 0001.
- Fail on latency or idle: xHCI stays in the kernel on MSI-X. Fail on restart: no ring-3 driver ships until teardown is fixed.

**S07-2, AMD-Vi under QEMU** (gates `amdvi`'s CI test plan; before P7).
- Build: a minimal AMD-Vi setup (IVRS, one device table entry, an I/O page table, the command buffer with COMPLETION_WAIT, the event log) with `-device amd-iommu`, virtio-net with `iommu_platform=on` (and ACCESS_PLATFORM negotiated) and qemu-xhci.
- Pass, under each accelerator separately: translated DMA works, and a deliberately bad DMA address logs an IO page fault, leaves a canary page intact and kills only the driver.
- Fail under both: `amdvi` is tested by host tests plus a hand check on the founder's PC at P7, with no model loaded.

**S07-3, uACPI glue** (gates section 8's AML; before P3).
- Pass: glue of at most 1,500 lines; no patch to uACPI's sources; namespace load at most 200 ms under TCG on q35; `\_S5` power-off and QMP `system_powerdown` succeed in 10 of 10 boots under each accelerator; `cargo xtask build` finds clang on the CI runner and on the Windows dev host (both unverified today).
- Fail: ACPICA under BSD-3-Clause, measured the same way.

**S07-4, a ring-3 NIC data path** (gates `netd`; before P7).
- Build: virtio-net driver plus smoltcp in a ring-3 process over KRNG rings, against today's kernel path. A 1 GiB TCP transfer to the xtask mock, five runs, medians.
- Pass: throughput at least 0.8x of the kernel path, and CPU per GiB at most 1.5x.
- Fail: the P7 NIC stays in the kernel behind the same `devkit` contract.

**S0, the founder's hardware** (not a spike; before P7 planning): the board model and its PCI IDs (question 3).

## Phases that depend on it

| Phase | Needs from this ADR |
|---|---|
| P1 | `pci` and `devmatch`, the vector allocator and MSI-X, `devkit` and `Zone::Below4G`, DeviceRecords and DEV_LIST, FADT reboot, and the action rows in section 11. |
| P2 | Block write, flush, info, queues and MSI-X completions for NVMe and virtio-blk (with ADR 0004); resend after a reset; the device ABI, KRNG v1 and `usbd` after S07-1; the i6300esb watchdog for ADR 0008's fault path. |
| P3 | `Display` for ADR 0005's compositor; typed input events from PS/2 and `usbd`; KRNG for `kai-surface 1`; uACPI power-off and the power button after S07-3. |
| P4 | DeviceRecords for the system monitor and settings apps. Nothing new in drivers. |
| P5 | virtio-net on MSI-X with the NAPI budget, for TLS downloads. |
| P6 | `audiod` on virtio-snd for the Linux micro-VM's audio (guest to `vmm` to `audiod`, ADR 0006); network throughput for video; KRNG for `vmm`'s exit ring. |
| P7 | PCI behind bridges on a real board, `amdvi` and `iommu=require`, `usbd` hubs and hot-plug for the AMD controllers, `netd` (rge or igc) after S07-4, `audiod` for HD Audio or USB Audio Class, x2APIC and more than 8 CPUs, the FADT 8042 flag, GOP on the RTX 4070 Ti, the board's AMD TCO watchdog (unverified on the founder's board). |
| P8 | Fuzzing of every device-facing parser and the DEV_CFG filter; the security review of the device ABI and teardown. |

The order of work after approval is in [0000-execution-plan.md](0000-execution-plan.md): S0; `pci` and `devmatch` (after #18 and #19 merge); vectors and MSI-X; `devkit` and zones; block write, flush and queues (with ADR 0004); the device ABI with `usbd`; uACPI; `amdvi` before P7. Design C's estimate for v0.1 is 4,500-6,000 lines in 10-12 PRs over P1-P3 (an estimate, not a measurement).

## Changes to recently merged and open work

| Work | Change | Files | Why |
|---|---|---|---|
| #19 USB input (open) | Merge as it is. Follow-up PR: move `controller.poll` out of the timer interrupt into a `usb` kernel thread that sleeps 2 ms between polls; about 30 lines. Add no hubs or hot-plug to the kernel glue. | kernel/src/idt.rs (the 4 lines #19 adds at :98-102), kernel/src/usb.rs:148-175 | Driver work must not run in interrupt context. HID parsing on every 1 kHz tick, with interrupts off, lengthens CPU 0's tick. Hubs belong in `usbd`. |
| #19, later | The `XHCI_CLASS` arm becomes a `devmatch` row (PCI PR). The below-4 GiB leak goes with `Zone::Below4G`, and `Dma::phys` is renamed `Dma::device` (devkit PR). After S07-1 passes, the crate runs in `usbd` and kernel/src/usb.rs shrinks to the INPUT_PUSH sink. | kernel/src/virtio/pci.rs, kernel/src/usb.rs:36-38, xhci/src/hal.rs:26-28 | One match table; IOVA naming; USB in ring 3. |
| #18 virtio-gpu (open) | No driver-model change. It must be rebased onto #17 first, since it edits kernel/src/console.rs, which #17 replaced with kernel/src/console/. Later: `Found.gpu` becomes a row; rename `display::Surface` to `display::BackBuffer`. | kernel/src/virtio/pci.rs, kernel/src/display/mod.rs:27 | One match table; `Surface` is ADR 0005's name for an `Shm` with a 4 KiB header page, and two meanings of one name would confuse the code. |
| #16 DHCP, DNS, ping (open) | None. The NIC stays in the kernel. Later, the `net` thread becomes the driver thread woken by MSI-X. | kernel/src/net/mod.rs:105 on the branch (vectors PR) | RESOLVE stays 37; new calls are numbered from 38 in claim order. |
| #13 parser hardening (merged) | Follow-ups: hostile tests for NVMe Identify (a new nvme/tests/hostile.rs), `xhci::descriptor` and `xhci::hid` (after #19), `gfxcore` EDID (after #18), PCI capability chains with the hop limit (with the PCI PR), and FADT and IVRS in acpitables/tests/hostile.rs (with those parsers). Bound the i8042 drain. | the files named; kernel/src/i8042.rs:92-95 | Device and firmware bytes are untrusted input. The drain would spin for ever on a board without an i8042, where port 0x64 usually reads 0xFF. |
| #15 seamless actions (merged) | No code change. The device and power rows go into `agentabi` in P1, not into `TIERS`. | shellcore/src/tiers.rs:35-43 (unchanged) | ADR 0002's registry replaces the interim table. |
| #17 Ember terminal (merged), #14 decode speed (merged) | None. | none | The console stays on GOP or virtio-gpu in ring 0. |
| docs | A follow-up PR adds the decisions row for this ADR; supersedes :58 (AML now through uACPI), :63 (MSI-X from P1) and the "bus 0 only" clause of :64; and closes docs/research/live-editable-system.md:81 ("Drivers in ring 3: Open"). Row :52 and the "no code is reused" clause of :12 are superseded by ADR 0009's reuse row, which this ADR relies on. | docs/decisions.md, docs/research/live-editable-system.md | Settled rows must not contradict the code. |

Sequencing: the PCI PR starts only after #18 and #19 merge, since all three rewrite `probe` and `Found` in kernel/src/virtio/pci.rs.

## Open questions for the founder

1. **Decided 2026-10-22:** C code in the build is allowed, so uACPI (MIT) goes into the kernel, built with clang, after spike S07-3. On Windows that means installing LLVM.
2. **Ported files and firmware blobs.** May drivers port permissive source files: FreeBSD's AMD IOMMU (BSD-2-Clause) and igc (BSD-2-Clause and BSD-3-Clause), OpenBSD's rge (ISC), Redox code (MIT)? This half is ADR 0009's question 4. May the image ship non-open, redistributable firmware: Realtek's RTL8125 microcode (distribution only) and, after v1.0, NVIDIA's GSP firmware? Without the microcode there is no 2.5 GbE wired network on many AM5 boards.
3. **Your hardware.** Please send your board's model and the output of `Get-PnpDevice -PresentOnly | Where-Object InstanceId -like 'PCI*' | Select-Object FriendlyName, InstanceId` in PowerShell. It only reads IDs; nothing of Kernel AI runs. Can the firmware's IOMMU (AMD-Vi) stay on? With `iommu=require`, ring-3 drivers do not start without it.
4. **Graphics.** Is a GOP-only display on the RTX 4070 Ti through v1.0 acceptable: one fixed mode, no refresh-rate or multi-monitor control? Or would you plug the monitor into the board's own output (the 7700X's integrated GPU) for Kernel AI? ADR 0005's question on CPU-only graphics is the same choice.
5. **Risk.** Until v1.0, a crash in the disk, VM network or display driver, or in ACPI, restarts the whole machine, while USB, the P7 network card and audio restart on their own. Is that acceptable for v0.1 to v0.3?
6. **GPL.** Confirm that Linux drivers (GPL-2.0) stay excluded even where they would save months, for example Wi-Fi.

## Sources

External (read on 2026-10-21 by this topic's research and judge unless dated otherwise):
- uACPI 7.0.0, MIT, released 2026-10-16: https://github.com/uACPI/uACPI
- ACPICA 20260930, Intel, BSD-3-Clause or GPL-2.0-only: https://github.com/acpica/acpica
- `acpi` 6.1.1, MIT/Apache-2.0, 2026-03-08: https://github.com/rust-osdev/acpi ; Redox issue 56 (updated 2026-08-26): https://gitlab.redox-os.org/redox-os/base/-/issues/56 ; Redox's fork, branch `redox-6.x`, named in https://gitlab.redox-os.org/redox-os/base/-/blob/main/Cargo.toml (main on 2026-10-21)
- `pci_types` 0.10.1, MIT/Apache-2.0, 2026-02-03: https://docs.rs/pci_types/0.10.1
- virtio-drivers 0.13.0, MIT, 2026-03-03: https://github.com/rcore-os/virtio-drivers (feature sets read in the 0.13.0 crate source: src/device/{blk.rs:14-18, net/mod.rs:201-205, sound.rs:754-756})
- FreeBSD AMD IOMMU, BSD-2-Clause, first commit 2024-11-02: https://github.com/freebsd/freebsd-src/tree/main/sys/x86/iommu
- AMD I/O Virtualization Technology (IOMMU) Specification, doc 48882 rev 3.10: https://docs.amd.com/v/u/en-US/48882_3.10_PUB (the link Linux's x86 IOMMU docs use; it returned 404 to a plain fetch on 2026-10-21, so the revision is as the research read it)
- OpenBSD `if_rge.c` rev 1.47 (ISC) and `rgemcode.h` rev 1.1, both 2026-10-13: https://github.com/openbsd/src/tree/master/sys/dev/pci and https://github.com/openbsd/src/tree/master/sys/dev/microcode/rge
- FreeBSD igc, BSD-2-Clause (`if_igc.c`) and BSD-3-Clause (`igc_api.c`): https://github.com/freebsd/freebsd-src/tree/main/sys/dev/igc ; rge(4), imported 2025-12-15: https://man.freebsd.org/rge(4) ; re(4) on 15.1 lists no RTL8125: https://man.freebsd.org/cgi/man.cgi?query=re&sektion=4
- Redox drivers, MIT, `main` on 2026-10-21: https://gitlab.redox-os.org/redox-os/base/-/tree/main/drivers ; March 2025 news: https://www.redox-os.org/news/this-month-250331
- Managarm, MIT: https://github.com/managarm/managarm
- NVIDIA open-gpu-kernel-modules COPYING, 615.78.08, 2026-10-07: https://github.com/NVIDIA/open-gpu-kernel-modules/blob/main/COPYING ; kernel_open README 555.42.02: https://download.nvidia.com/XFree86/Linux-x86_64/555.42.02/README/kernel_open.html
- nova-core status at LPC 2026: https://www.phoronix.com/news/Nova-Driver-LPC-2026
- Genode licences and 26.08 release (2026-08-25): https://genode.org/about/licenses , https://genode.org/news/genode-os-framework-release-26.08
- seL4 sDDF 0.7.0, BSD-2-Clause, 2026-08-19: https://github.com/au-ts/sddf
- Hubris reference (MPL-2.0): https://hubris.oxide.computer/reference/
- Tock design, release-2.3.0 (2026-10-08): https://book.tockos.org/doc/design
- Fuchsia driver framework: https://fuchsia.dev/fuchsia-src/concepts/drivers/driver_framework
- Linux NAPI: https://docs.kernel.org/networking/napi.html ; VFIO: https://docs.kernel.org/driver-api/vfio.html
- Shadow drivers, OSDI '04: https://www.usenix.org/conference/osdi-04/recovering-device-drivers
- QEMU `hw/i386/amd_iommu.c` at v8.2.0: https://gitlab.com/qemu-project/qemu/-/blob/v8.2.0/hw/i386/amd_iommu.c
- VirtualBox `modifyvm --iommu`: https://www.virtualbox.org/manual/topics/vboxmanage.html

Repo (main at ec5ecff unless a branch is named): docs/roadmap.md:47, :131, :139, :146, :158; docs/decisions.md:12, :14, :52, :54, :57, :58, :63, :64, :70, :123, :211, :214; xtask/models.json:156; kabi/src/lib.rs:25-45; docs/research/live-editable-system.md:81, :215, :251; docs/research/rust-kernel-foundation.md:65; kernel/src/virtio/pci.rs:62-71, :78-109, :145; kernel/src/virtio/hal.rs:95-96; kernel/src/virtio/split.rs:38, :95; kernel/src/idt.rs:95-122; kernel/src/apic.rs:9-15, :62-63; kernel/src/percpu.rs:12; kernel/src/gdt.rs:78; kernel/src/process/mod.rs:54, :293-294; kernel/src/process/syscall.rs:21; kernel/src/ipc.rs:13-14, :29; kernel/src/memory/mod.rs:36; kernel/src/memory/user.rs:9-11; kernel/src/main.rs:245-249, :453-460; kernel/src/i8042.rs:92-95; kernel/src/port.rs:9-31; kernel/src/nvme/controller.rs:19, :126-127; kernel/src/block.rs:13-28; mm/src/frames.rs:110-119; acpitables/src/lib.rs:1-4; kabi/src/lib.rs:82-108, :101-103, :160; kabi/src/call.rs:86-92; supervise/src/lib.rs:158-164; shellcore/src/tiers.rs:35-43; testkit/hostile.rs; .github/workflows/ci.yml:48-59; user/src/bin/hog.rs. Branches: origin/dane/usb-input-83eb7c (0717caa) xhci/src/lib.rs:5-8, xhci/src/hal.rs:26-28, :102-130, xhci/src/driver/mod.rs:132, kernel/src/usb.rs:27-47, :36-38, :148-175, kernel/src/idt.rs:98-102; origin/dane/virtio-gpu-a5daaa (af7635d) kernel/src/display/mod.rs:1-12, :27; origin/dane/net-dhcp-dns-b5bdb7 (131aa77) kabi/src/call.rs:134, kernel/src/net/mod.rs:105. Topic inputs: D:/kai-targets/p0/work/07/{judgement,design-A,design-B,design-C,research-code,research-prior}.md and D:/kai-targets/p0/work/decision-sheet.md.
