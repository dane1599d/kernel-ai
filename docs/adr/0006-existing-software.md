# ADR 0006: Running existing software: the browser path

Proposed, 2026-10-21. For the founder's approval; nothing here is decided until approved.

Repo citations are `path:line` on main at ec5ecff unless a branch is named. External facts were read or rechecked on 2026-10-21 by the topic's research, design and judge passes, or by this ADR's author; anything not rechecked says "unverified". Nothing was built, run or spiked for this ADR, and no model ran anywhere. Effort figures are the designs' estimates.

## Context

The founder wants "pull up my browser and go to youtube" to just happen (docs/decisions.md:203), and a production OS that people have a reason to use. The roadmap's P6 asks for "a Linux ABI layer ... or a ported browser engine", with the exit test "A static Linux binary runs, then a GUI app, then a browser plays a YouTube video in the VM" (docs/roadmap.md:145). The choice is the open row docs/decisions.md:206. P6 is one of the two long poles (roadmap.md:158), and v0.2 is P5-P6.

What the kernel offers a foreign program today:

- **Loading.** `uelf` refuses `PT_DYNAMIC`, `PT_INTERP` and `PT_TLS` (uelf/src/lib.rs:132) and any W+X segment (:154-157). The loader puts nothing on the stack: RSP is `STACK_TOP - 8` (kernel/src/process/loader.rs:133-138). FS_BASE is written once per CPU (kernel/src/cpu.rs:130), so there is no TLS.
- **Calls.** The registers are Linux's (kabi/src/lib.rs:5-7) but the numbers collide: EXIT is 0 and WRITE is 1 (kabi/src/call.rs:4, :6). The last call on main is 36 (:125); RESOLVE claims 37 on the open DHCP/DNS branch.
- **Memory.** User pages are never unmapped and nothing sends a user TLB shootdown (kernel/src/memory/user.rs:5-12); futexes rely on that (kernel/src/process/futex.rs:15-17). Every data page is NX (memory/user.rs:137-138). Admission has no overcommit: the budget is half the free frames at boot (kernel/src/main.rs:481-482).
- **Limits.** 16 processes and 512 MiB each (kernel/src/process/mod.rs:54-58), 64 threads system-wide (sched/src/lib.rs:39), 8 pinned workers per process (kabi/src/thread.rs:15) and only `llm` may start them (`cap::THREADS`, kabi/src/lib.rs:104-106), a 32 MiB kernel heap (kernel/src/memory/heap.rs:14), and a 2,000 ms runaway burst by default (kernel/src/process/supervision.rs:34; 0 means none, kernel/src/process/burst.rs:25).
- **No signals, sockets, files, shared memory, audio or wall clock.** QEMU and VirtualBox get no audio device (xtask/src/qemu.rs:79-94; xtask/src/vbox/mod.rs:335), and audio appears only in P7 (roadmap.md:146), yet the P6 exit test plays a video with sound.
- **Memory beside the model.** Qwen3-4B is 2,497,281,120 bytes (xtask/models.json:156), and `run --local` gives the VM 3.5 GiB (docs/decisions.md:182).
- **Where we can test.** The founder's PC has no KVM inside WSL2 (docs/decisions.md:18), so the QEMU 10.2.1 unpacked there on 2026-10-21 runs TCG only. VirtualBox runs on Hyper-V (docs/decisions.md:211). CI runs QEMU with KVM and TCG on GitHub's ubuntu-24.04 runners (.github/workflows/ci.yml:48-69).

Since the research ran, main merged #13 (parser hardening, `testkit/hostile.rs`), #14 (AVX2 attention), #15 (seamless actions: `shellcore/src/{tiers,actions,undo,show,log}.rs`) and #17 (the "Ember" terminal). #16 (DHCP, DNS, ping), #18 (virtio-gpu 2D) and #19 (USB input over xHCI) are green and open.

## Options considered

Three designs were judged (scores out of 10). Every route plays video on the CPU in the VM: no GPU acceleration before P7.

**A. A Linux ABI layer in our kernel (score 6, kept as the fallback).** A per-process "Linux personality" picked at syscall entry, served by a host-tested `lxabi` crate, runs Alpine's own Firefox ESR, shown through a Wayland bridge.
- Evidence for: WSL1 reached about 235 calls by June 2016 (research figure, not rechecked); gVisor implements 290 of 352 amd64 calls (Apache-2.0) and ships syscall tests in 296 C++ files (`test/syscalls/linux` at release-20261012.0); Asterinas (MPL-2.0, Rust) runs X.Org on llvmpipe (research, not rechecked). Mozilla maintains the browser, and a whole distribution's catalogue would run.
- Evidence against: Microsoft moved WSL to a VM for "full system call compatibility". Asterinas reached over 210 calls in three years with more than 50 people (research figures), and nobody reports a browser on it. A reworks invariants the M10 proofs ran on: never-unmap (memory/user.rs:5-12), futexes (futex.rs:15-17), no overcommit (main.rs:481-482), 16 processes, the runaway kill. It puts tens of thousands of lines of Linux semantics in ring 0, and its `LEVEL` becomes a lasting promise once users install apps.
- Cost: about 87 engineer-weeks (about 20 PM), 18-32 PM with the tail, 20-46 PM by the independent research estimate. Only a static binary lands by v0.1.
- Why it lost: the most kernel risk and the longest tail. It stays the fallback because it needs no nested virtualisation.

**B. A native Ladybird port (score 4, rejected).** New handle objects in the kernel (TLS, threads, mappings, shared memory, channels with handle passing, poll, spawn, sockets), mlibc v7.0.0 (MIT) for POSIX, Ladybird (BSD-2) with JIT off and dav1d, libvpx and libopus in place of FFmpeg.
- Evidence for: the cleanest licence and the deepest integration (typed browser actions in our own process).
- Evidence against: Ladybird has taken no public pull requests since 2026-06-05 and does not treat forks as a review queue, so the port is a permanent out-of-tree fork of a pre-alpha, mostly C++ project. LibJS has an optimising JIT, on by default only for the Linux and macOS calling conventions, so a Kai build would interpret JavaScript at an unmeasured cost. `ENABLE_CRANELIFT_JIT` defaults to ON everywhere and must be set OFF. Ladybird registers only FFmpeg decoders (LGPL), AudioToolbox on macOS and a remote video decoder, so B writes its own decoder adapters. B also needs a patched LLVM triple, an out-of-tree Rust `std` and two TLS stacks (OpenSSL in the browser, rustls in the OS). Only ported software runs, so the "static Linux binary" leg fails as written. It conflicts with ADR [0003](0003-app-model.md), which makes apps Wasm only.
- Cost: 18-28 PM plus a standing rebase cost; its kernel steps alone are 4.25-5.25 PM.
- Why it lost: highest debt. Its kernel objects are good and are grafted below for native services.

**C. Linux in a micro-VM (score 7, chosen).** Our own AMD SVM hypervisor runs one Linux guest with a stock browser; a ring-3 `vmm` emulates virtio devices; a guest agent carries the AI's actions. This is the WSL2 and ChromeOS Crostini shape ("The VM is our security boundary", Crostini docs).
- Evidence for: every Linux app runs at upstream versions, and the browser brings its own JIT, codecs, MSE, TLS and sandbox. The kernel keeps W^X and every proven invariant. It adds one versioned ABI with one client, and nothing leaks into the app model. QEMU's TCG offers SVM with NPT but not NRIPS (QEMU 10.2.1 and master), so it can be tested functionally on the founder's PC.
- Evidence against: in a VM it needs nested virtualisation. The VirtualBox manual says nested paging is not passed through to guests, and the founder's PC is TCG-only locally. The guest needs about 1.5 GiB beside the model. We ship and patch a GPL guest image. Linux apps stay foreign: their own theme, and window-level control.
- Cost: 7-15 PM to the exit test (design C and the research agree), plus 1-2 PM for a VMX backend.
- Why it won: the lowest kernel debt and the fastest route to a browser that people already trust. The risks are measurable in a 4-week spike.

**The guest base.** The judge picked Alpine. The decision sheet puts real apt and dpkg inside the guest (ADR [0011](0011-packages.md) leaves the distribution to this ADR), so the guest is Debian. Evidence, read 2026-10-21:

| | Alpine (edge) | Debian 13 "trixie" |
|---|---|---|
| Firefox ESR | 140.14.0-r0 | 153.5.1esr-1~deb13u1 (installed size 301,518 kB) |
| Package manager | apk-tools (GPL-2.0) | apt and dpkg (GPL-2+), as ADR 0011 assumes |
| libc | musl: fewer calls, so a smaller fallback A | glibc: vendor `.deb` files run; Chromium 155.0.8059.79-1~deb13u1 is a package |
| Stock kernel | `linux-virt` virtio options unverified | `linux-image-amd64` 6.12.113-1: `VIRTIO_PCI=y`, `VIRTIO_MMIO=m`, `VIRTIO_MMIO_CMDLINE_DEVICES` not set, `X86_MPPARSE=y` |
| Security fixes | apk | Debian security, through apt |

**Open-source reuse** (licence classes per ADR [0009](0009-licence.md) and the 2026-10-21 rule):

| Project | Version, licence | Use here | Class |
|---|---|---|---|
| crosvm | main ccfda98, BSD-3-Clause | port userspace LAPIC, IOAPIC and irqchip (`devices/src/irqchip/`), virtio-gpu protocol, virtio-input | free |
| Cloud Hypervisor | v54.0 (2026-10-14), Apache-2.0 and BSD-3-Clause | port the x86 MMIO emulator, PCI transport, block, net and console devices | free |
| Firecracker | v1.17.0 (2026-09-10), Apache-2.0 | boot recipe, MP table, minimal device set | free |
| linux-loader 0.14.0, virtio-queue 0.18.0; vm-superio 0.8.2 | Apache-2.0 AND BSD-3-Clause; Apache-2.0 OR BSD-3-Clause | they import `std`, but only `core`-equivalent parts, so porting is mechanical | free |
| iced-x86 | 1.21.0, MIT, `no_std` | linked: instruction decode | free |
| ext4-view | 1.0.0, MIT OR Apache-2.0 | linked into `vmm`: read the guest's `/vmlinuz` and `/initrd.img` | free |
| virtio-drivers | 0.13.0, MIT (kernel/Cargo.toml:33) | `device::sound` for `audiod` | free, in tree |
| sommelier, rutabaga_gfx; WSLg | BSD-3-Clause; MIT | later per-window bridge (reference or port) | free |
| libkrun | v1.19.6 (2026-09-29), Apache-2.0 | reference only (library VMM) | free |
| Debian 13 guest | GPL-2.0 kernel, GPL apt and dpkg, LGPL glibc, Firefox (Alpine declares "GPL-3.0-only AND LGPL-2.1-only AND LGPL-3.0-only AND MPL-2.0"; Debian's copyright file not read) | the guest, run as separate programs | ask (question 1) |
| mmdebstrap; e2fsprogs `mke2fs` | MIT (script header); GPL-2 (Debian copyright file) | CI-only image build, non-crate tools | ask (question 2) |
| musl | 1.2.5 in Rust's `x86_64-unknown-linux-musl` since Rust 1.93, MIT | statically linked into `kai-guest` (C in the image) | ask (question 2) |
| gVisor syscall tests | release-20261012.0 (2026-10-16), Apache-2.0 | their call list costs fallback A in the spike | spike only |
| Asterinas | 0.19.0 (2026-10-12), MPL-2.0 | file-by-file port, only if fallback A is taken | ask then |
| Ladybird; mlibc | BSD-2-Clause; v7.0.0 (2026-08-02), MIT | route B | rejected |
| Servo | MPL-2.0; desktop media only through GStreamer (LGPL-2.1, research); no MSE (issue #22416, open on 2026-10-21) | browser engine | rejected |
| Chromium natively | BSD-3; a new OS needs an Ozone fork; FreeBSD carries 1,661 patch files (research, not rechecked) | browser engine | rejected; an apt package in the guest |
| NetSurf; Maestro | GPL-2.0; AGPL-3.0 | none | never |

"Free" here is the licence class. The ports (crosvm, Cloud Hypervisor, Firecracker and the three rust-vmm crates) copy source files rather than add crates; whether the founder's crate rule covers copied files is ADR 0009's question 4. Each PR lists what it copies.

## Decision

In plain words: Kernel AI runs a small, hidden Linux system inside itself, and the browser (Firefox) runs in that Linux. Our kernel gains a hypervisor, the part that runs a second operating system safely in a box, and a small helper inside Linux lets the AI open pages, read them and play or pause video. "Go to youtube" just happens; typing into pages, downloads and running programs ask first. If a 4-week spike shows this is too slow or will not run on our test machines, we fall back to teaching our own kernel to run Linux programs directly.

1. **Route.** Linux in a micro-VM (design C) is P6's route. Fallback: design A (item 12). The Ladybird port is rejected and re-measured only if both A and C fail.

2. **The hypervisor, in the kernel.** AMD SVM first.
   - `cpufeat` gains SVM (CPUID 0x8000_0001 ECX), and from leaf 0x8000_000A NPT, NRIPS, DecodeAssists, AVIC and the ASID count. They go on the existing boot line (kernel/src/fpu.rs:163; struct at cpufeat/src/lib.rs:69-92).
   - A host-tested `no_std` crate `svm` holds the VMCB layout, exit codes, intercept sets, EVENTINJ encoding and NPT entry builders (AMD64 APM vol. 2, pub. 24593, ch. 15; revision not checked).
   - `kernel/src/vm/` sets EFER.SVME beside EFER.SCE (kernel/src/cpu.rs:42-44, :122-123) and VM_HSAVE_PA on each CPU. Each vCPU is a kernel thread owned by `vmm`, counted in `live_threads` as L6 workers are (kernel/src/process/workers.rs:158), so the last thread out still frees the space. The loop loads guest XSAVE state into the thread's area (kernel/src/fpu.rs:37), then CLGI, VMRUN, STGI.
   - The kernel handles CPUID (a table fixed at create), an MSR allow-list, HLT, the x2APIC and TSC-deadline timer, and IOAPIC and MSI routing (ported from crosvm). Every other exit goes to `vmm`.
   - Port I/O and MMIO exits are decoded with iced-x86 and emulated by a port of Cloud Hypervisor's x86 emulator. The decoder fetches instruction bytes itself, because TCG offers no NRIPS or DecodeAssists.
   - Guest time is never charged to the runaway burst; the vCPU is still preempted every tick. `vmm`, and so its vCPU threads, carries the per-process `background` label of the `classes` policy (docs/decisions.md:39, :177), so the model and the UI keep priority.
   - A VMX backend is due before P8, or inside P6 if spike S06-1 finds mostly Intel runners.

3. **The ABI** (versioned from the first release, roadmap.md:131).
   - `kabi::vm` version 1: one call `VM` with the operation in RDI (`create{version, vcpus, ram_mib, flags}`, `set_regs`, `run`, `irq`, `stop`, `destroy`). Its number is given in claim order when the PR lands (docs/decisions.md:123).
   - A `Vm` handle kind (ADR [0001](0001-capabilities.md)). Its kind-ceiling bit is the next free one in claim order. Only the service `vmm` running the program `vmm` may hold it, as only `llm` may hold `MODEL` today (kabi/src/lib.rs:101-103).
   - Guest RAM is one ADR 0001 `Shm` object, allocated in full at create, mapped read-write and NX into `vmm` at a fixed PML4 slot `GUEST_BASE` (as `MODEL_BASE`, kabi/src/lib.rs:249), and used as NPT backing. It is never unmapped while the VM lives, so the never-unmap rule holds and no user shootdown is needed. It is its own reservation, outside the 512 MiB per-process cap, and is refused when it would exceed the page budget.
   - Exits and doorbells travel on an ADR [0007](0007-drivers.md) KRNG v1 ring of a new kind `VMEXIT`, claimed in order in `kabi::ring` (0007 adds it to its list). Each exit is one standard 64-byte request: `tag` = exit sequence, `op` = exit kind (Mmio 1, Pio 2, Doorbell 3, Shutdown 4, TripleFault 5), `offset` = guest address or port, `len` = access size, `flags` bit 0 = write, `args[0]` = vCPU, `args[1]` = write data, `buffer` 0. `vmm` answers with the standard 16-byte response, `status` 0 or a negative `kabi::Error`. Read data goes in the ring's data area (`buffer` 0), in the 8-byte slot that matches the request's ring slot (`tag mod entries`, since tags count up by one), with `len` = size. A write to a virtio notify register is posted as a doorbell without stopping the vCPU. Network frames use two more KRNG rings.
   - No executable memory is created at run time in the OS before v1.0. JITs run only inside the guest, and there is no `Jit` kind.

4. **`vmm` and its devices.** `vmm` is a `no_std` ring-3 program, single-threaded and event-driven. It holds the `Vm` handle, the RAM `Shm`, File handles for the guest disks (ADR [0004](0004-storage.md)), a per-VM network grant (ADR 0001), one `Surface` (ADR [0005](0005-graphics.md)) and a route to `audiod`.
   - Transport: virtio 1.2 over PCI (modern, MSI-X, config space through ports 0xCF8/0xCFC), not virtio-mmio. Debian's stock kernel builds `VIRTIO_PCI=y` and leaves `VIRTIO_MMIO_CMDLINE_DEVICES` unset, so MMIO devices would need ACPI or our own kernel build. Its virtio block, GPU, input and sound drivers are modules, loaded from Debian's initrd. Keeping the stock kernel lets Debian's security team patch it. Porting the PCI transport adds about 2-4 weeks (estimate).
   - Devices: virtio-blk three times (root, read-write; home, read-write; `kai-tools`, read-only), virtio-console multiport (`hvc0` and the agent port), virtio-net, virtio-gpu 2D with EDID, virtio-input (keyboard and absolute tablet), virtio-snd (PCM out), and an 8250 serial port for the boot log and the CI pass line.
   - Boot: no firmware and no ACPI. `vmm` reads `/vmlinuz` and `/initrd.img` from the root disk with ext4-view, falling back to `/vmlinuz.old`. It loads the bzImage with a zero page, e820 map and command line (linux-loader port), and writes an MP table for the CPUs and IOAPIC (Firecracker port). Debian builds `X86_MPPARSE=y`, but whether its kernel boots SMP from an MP table with no ACPI is unverified; spike S06-3 checks the boot.
   - Defaults: 4 vCPUs and 1,536 MiB. The VM starts on first use (`browser_open` runs `vm_start` first) and stops after 30 minutes with no windows (an initial value), freeing its RAM.

5. **The guest: Debian 13 as the `linux-layer` package.**
   - Contents: `linux-image-amd64` (6.12.113-1 today), systemd, `cage` 0.2.0-2 (a kiosk Wayland compositor), `firefox-esr` 153.5.1esr-1~deb13u1, fonts, apt and dpkg. Installed with `--no-install-recommends`, so FFmpeg's `libavcodec` (a Recommends of firefox-esr) is absent and H.264 and AAC are off (question 7). VP9 comes through `libvpx9` (a Depends); AV1 and Opus from Firefox's own tree (unverified for Debian's build; S06-2 plays both).
   - Build: xtask runs mmdebstrap in CI against snapshot.debian.org at a pinned timestamp with `SOURCE_DATE_EPOCH` (its README says the result is then bit-by-bit reproducible; unverified for our set), then `mke2fs -d`. The pin list is `xtask/guest.json`, in the style of xtask/models.json: snapshot time, packages, image length and SHA-256. It is a build input for xtask, not a manifest. Image target: at most 1 GiB unpacked (S06-2 records it).
   - Delivery: a signed `linux-layer` package under the `packages` role of the one TUF repository `dane1599d/kernel-ai-updates`, checked by the one `kaituf` client (ADR [0008](0008-updates.md), ADR 0011). Its `kai-package 1` header carries `blob <sha256> <length>`; the bytes come in 2 MiB chunks against ADR 0008's signed `kai-chunks 1` list, to `/system/pkg/blobs/<sha256>` (ADR 0011).
   - Disks: the blob is sealed and only seeds. At first start (and on "reset Linux") `vmm` copies it to `root.img`. The writable disks are user data in the kaifs volume `linux` at `/home/linux/` (`root.img`, `home.img`; ADR 0004 §5's `linux` row), so their snapshots do not pin blocks in the user's `home` volume. No overlay of a read-only base: dpkg's database would copy up into the upper layer and shadow every later base update.
   - Updates: inside the guest by Debian's apt, as the action `linux_upgrade`, after an ADR 0004 `package` snapshot of the `linux` volume; `undo` restores it. That is ADR 0008's update row for the guest. `apt install X` follows ADR 0011 (`foreign debian X <version>` in a generation). A new base blob only seeds new installs and resets.
   - Consistency: before any snapshot, `vmm` sends kvp/1 `Sync{freeze}` (the guest runs `fsfreeze`), then thaws after.
   - `kai-tools`: a read-only ext4 image of a few MiB inside the OS slot's `kai.bundle` (ADR 0008), mounted at `/opt/kai`. It carries `kai-guest`, so the guest agent always matches the host's kvp/1.

6. **The guest agent and kvp/1.** `kai-guest` is a static Rust binary for `x86_64-unknown-linux-musl`, started by a systemd unit from `/opt/kai`. It speaks **kvp/1** on virtio-console port `org.kernelai.agent.0`: a u32 little-endian length, then JSON, at most 64 KiB a frame. `Hello` carries the protocol version, the `kai-tools` build and the layer's base SHA-256.
   - Host to guest: `Hello`, `OpenUrl{id,url}`, `Close{id,window}`, `ListWindows{id}`, `Media{id,op}`, `PageText{id,window,max}`, `Run{id,path,args}`, `Sync{id,freeze}`.
   - Guest to host: `Done{id,result}`, `Window{id,title,app}`, `Output{id,bytes}`.
   - The guest only answers and reports. It can never request a host action, declare a row or raise a tier. Everything it sends is `data` provenance (ADR 0001) and a `Data` value (ADR [0002](0002-agent-actions.md)).
   - Firefox is driven through WebDriver BiDi on its remote agent (`--remote-debugging-port`), which Mozilla restricts to loopback and to connections with no `Origin` header by default. BiDi on that port without geckodriver is unverified until S06-2. Media play and pause go through MPRIS, or BiDi if MPRIS is absent under cage (unverified).

7. **Actions** (rows in ADR 0002's `agentabi`, declared by the host, names per the decision sheet; prompt lines in ADR [0010](0010-skill-pack.md)'s OS groups, inside its 1,024-token budget for core rules and OS rows). Tiers are what ADR 0002's rules derive from each row's inverse:

| Row | Arguments | Tier | Inverse (ADR 0002 kind) |
|---|---|---|---|
| `vm_start` | none | Run | `Compensate`: stop the VM if this call started it |
| `browser_open` | `url` (from the request; a named site such as "youtube" through ADR 0002's curated host table) | Run when the user named the site (ADR 0002 consent); otherwise Ask | `Compensate`: close the tab it opened |
| `browser_read` | `window` | Run, returns `Data` | `Read` |
| `window_close` | `window` | Run for a Firefox tab (inverse: reopen its URL); Ask for any other Linux window, because kvp/1 cannot report unsaved state (ADR 0005 §6) | `Compensate` for a Firefox tab: reopen its URL (page state is lost); `None` for any other window |
| `media_play`, `media_pause` | `window` | Run | `Compensate`: each other |
| `app_open`, `app_close` | `app` (from the guest's `.desktop` list, which is data) | Run | `Compensate`: each other |
| `linux_upgrade` | none | Run | `Snapshot`: the `package` snapshot |
| `browser_type`, `browser_submit`, `browser_download` | `window`, text or target | Ask | `None` |
| `linux_run` | `path`, `args` | Ask | `None` |

   A name outside the registry is refused (ADR 0002 D2). The guest can never add rows. "Pull up my browser and go to youtube" becomes `{"name":"browser_open","arguments":{"url":"https://www.youtube.com"}}`, and code writes the result line ("Done: opened www.youtube.com in Firefox. undo"). Budget under KVM, all estimates until measured:

| Stage | Budget | Basis |
|---|---|---|
| Enter to status line | at most 100 ms | ADR 0002 |
| Prefill of State and request, decode of the tagged call | about 2-4 s for the whole warm action | measured on the founder's 7700X with generated 4B-shaped weights, no real model (docs/decisions.md:171, :174): prefill about 91 tokens/s on 8 threads, decode about 11-13 tokens/s; ADR 0002's S02-2 measures it in Kernel AI under KVM on a GitHub runner |
| `OpenUrl` on a running VM to the tab's first frame | at most 1 s | S06-2 measures |
| `vm_start` when the VM is down, to the first frame | at most 10 s | S06-2 bar |

8. **Display, input, audio and network.**
   - Display in v0.2: cage draws on the guest's virtio-gpu 2D scanout. `vmm` copies each `TRANSFER_TO_HOST_2D` rectangle into one ADR 0005 `Surface` (XRGB8888, damage rects in its 4 KiB header), shown as one window through `kai-surface 1`. At 1080p30 that is about 249 MB/s of copies (1920 x 1080 x 4 x 30).
   - Later, a P6 follow-up agreed with ADR 0005: one host window per Linux window through a guest-side, sommelier-style bridge over virtio-gpu cross-domain. 0005's `wlbridge` applies only if fallback A is taken.
   - Input: the compositor's events on the surface's event ring go to virtio-input as keys and absolute tablet positions. `vmm` turns ADR 0007's HID usage codes into Linux evdev codes, as ADR 0005 expects.
   - Audio: virtio-snd from the guest to `vmm`, then to `audiod`, ADR 0007's ring-3 driver on virtio-drivers 0.13 `device::sound`, for QEMU's `virtio-sound-pci`. HDA (VirtualBox and real hardware) follows in P7.
   - Network: `vmm` passes frames to a NAPT in `netcore` (from #16). The guest is 10.0.3.2/24, with gateway and DNS at 10.0.3.1, which `netcore` forwards to the lease's DNS server. The per-VM grant (ADR 0001, and its question 3 on broad web access for the VM) is checked there. Firefox does its own TLS (NSS), so browsing does not wait for P5's rustls; the layer's verified download still needs P5.

9. **Memory.** A VM that runs the browser beside Qwen3-4B needs about 7 GiB (arithmetic, unmeasured; question 8). ADR [0010](0010-skill-pack.md) §4 raises the 4B's VM from today's 3,584 MiB to 3,840 MiB from P1 (est.). That plus 1,536 MiB of guest is about 5.25 GiB before `vmm` and its buffers. But the model is loaded outside every quota (kernel/src/main.rs:246-248) and admission then allows processes only half the free frames (:481-482), so under that rule the guest's 1,536 MiB costs about 3 GiB of VM memory, about 6.9 GiB in all with `vmm` (est.). Step 4 settles how the guest's reservation counts against the half rule. Threads: 4 vCPUs plus `vmm` beside `llm`'s 9 should fit the 64 slots (sched/src/lib.rs:39), which also hold kernel threads; step 3 checks it.

10. **Exit test under this route.** (1) A static musl binary on `home.img` runs through `linux_run` (or the typed `linux <path>`), its output in our terminal. (2) A GUI app (`foot` under cage) shows in the window and takes keys. (3) Firefox plays a self-hosted MSE/DASH VP9+Opus page with sound in CI under KVM; youtube.com is a manual check. Leg 1 runs inside the guest unless the founder rules otherwise (question 3).

11. **Grafts from B.** Its kernel objects, minus the browser: per-thread FS_BASE and `PT_TLS`, a SysV initial stack, VM_MAP, VM_UNMAP and VM_PROTECT with user TLB shootdowns, POLL, and channels with handle passing. They are built for native services and P3 surfaces under ADRs 0001 and 0005 and M10b, named in ADRs and numbered in claim order. They are not P6 work.

12. **Graft from A, and the fallback.** S06-2 records A's census, and it is checked against gVisor's syscall test list to cost the fallback. If S06-2 or S06-3 fails, the fallback is design A as a Linux personality (`lxabi`, host-tested) mapped onto the native handle objects, running the same Debian root. glibc may call `clone3` and `rseq`; whether it falls back cleanly on ENOSYS is unverified.

13. **Work order** (sizes for one engineer: S at most 2 weeks, M 2-4 weeks, L 1-2 months):

| # | Step | Size | Needs |
|---|---|---|---|
| 0 | Spike S06-1 to S06-3 | 4 weeks | none |
| 1 | `cpufeat` SVM bits on the boot line; CI prints the runner's vendor and nested flag | S | none (may land in v0.1) |
| 2 | `svm` crate with host tests | M | 1 |
| 3 | `kernel/src/vm/`: VMRUN loop, CPUID, MSR, HLT, XSAVE, ASIDs; boot case `vm-hello` (TCG and KVM) | M | 2 |
| 4 | `kabi::vm`, the `Vm` kind, RAM `Shm`, the KRNG exit ring | M | 3, ADRs 0001 and 0007 |
| 5 | x2APIC, TSC-deadline timer, IOAPIC, MSI (crosvm port) | L | 3 |
| 6 | Exit decode and emulation (iced-x86, Cloud Hypervisor port) | M | 4 |
| 7 | `vmm`: boot from the ext4 root, 8250, MP table; boot case `vm-linux` prints `kai-guest ok` under TCG | M | 5, 6 |
| 8 | PCI transport, virtio-blk, virtio-console, kvp/1 crate (host-tested, fuzzed) | M | 7, P2 |
| 9 | virtio-net and the `netcore` NAPT | M | 8, P5 |
| 10 | virtio-gpu 2D, virtio-input, the foreign `Surface` | M | 8, P3 |
| 11 | virtio-snd device and `audiod` | L | 8 |
| 12 | `xtask/guest.json`, the layer build, `kai-guest`, `kai-tools`, verified download | M | 8, P2, P5 |
| 13 | Rows, tiers and undo; P6 exit cases | M | 9-12, P1 |
| later | Per-window bridge, file sharing, clipboard; VMX backend | L each | P3 |

## Consequences

- Every Linux app, not only a browser, runs at its upstream version from v0.2, with Debian's security fixes through apt.
- The kernel keeps W^X by construction, never-unmap, no overcommit and the runaway rule. The browser's needs do not reach our syscall ABI or the app model; Linux apps are a foreign class outside `kai:app` (ADR 0003).
- Untrusted web content sits behind two walls: Firefox's own sandbox, inside a guest isolated by NPT. Device bugs land in ring-3 `vmm`, which holds only its handles.
- New ring-0 attack surface: VMEXIT handling, NPT and instruction emulation. Fuzzing and a review are P8 gates.
- Without nested SVM, the in-VM browser cannot be fast in VirtualBox. The founder may first see full speed on real hardware (P7).
- All apps in the guest share one principal and the VM's grants. A separate trust domain means a second VM, which costs RAM.
- Linux apps look like Linux apps, and in v0.2 they share one window. Clipboard, drag and drop, file sharing and per-window management come later.
- Guest disk writes go through kaifs's copy-on-write records (up to 128 KiB, with smaller records for VM disk files planned in P6, ADR 0004); write amplification for a 1 GiB image is unmeasured until P2's spikes.
- We carry a second operating system: a base layer to keep current and `kai-guest` to maintain for as long as Linux apps are offered.

## What it costs to change later

| Choice | Cost to change | Why |
|---|---|---|
| `kabi::vm` | Low | One client, versioned |
| kvp/1 | Low | Versioned; `kai-guest` ships in the same OS slot as the host side |
| virtio device set and PCI transport | Low | Standard; Linux drivers are upstream |
| Action names | Low | Provider-neutral: `browser_open` survives a move to route A or B |
| Debian as the guest | Medium | Users' apt-installed software ties to Debian; moving distribution migrates `home.img`, not `root.img` |
| Hypervisor in ring 0 | Medium | One module behind one kind; removable, and reusable later for isolating untrusted apps |
| C to A later | Medium | The Debian root, surfaces and rows carry over; the hypervisor is sunk |
| Shipping a GPL guest image | High, ongoing | Source-offer duties and update cadence, unless the founder picks assembly on the device |

## Spikes that settle it

P0-06, about 4 weeks for one engineer. It runs on GitHub Actions `ubuntu-24.04` runners (4 vCPUs, 16 GB, KVM; vendor mix unknown) and under local WSL2 QEMU 10.2.1 with TCG for function only. VirtualBox gets one manual look on the founder's PC. No model disk is attached and no LLM runs anywhere. The stream is a self-hosted MSE/DASH page with VP9+Opus and AV1+Opus at 720p30 and 1080p30, 60 s each, never youtube.com in CI. Dropped frames come from `getVideoPlaybackQuality()`.

| Part | Build and measure | Pass | Fail means |
|---|---|---|---|
| S06-1 nesting census (2 days) | A CI matrix of 20 runs prints the CPU vendor, `/dev/kvm` and `kvm_amd`/`kvm_intel` `nested`. A spike branch adds the SVM, NPT, NRIPS and VMX bits to `cpufeat` and logs them under KVM `-cpu host` and under TCG. Once in VirtualBox with nested VT-x/AMD-V on. | SVM with NPT in at least 18 of 20 KVM runs, and always under TCG | No KVM path: the in-VM browser cannot be shown before P7. Take the fallback, or the founder accepts that (question 4). Mostly Intel runners: VMX moves into P6 (+1-2 PM). |
| S06-2 ceiling and census (1 week) | Plain QEMU/KVM runs the Debian guest of item 5 (cage, Firefox ESR 153, 4 vCPUs, 1,536 MiB, virtio-*-pci devices, virtio-gpu 2D without virgl, `virtio-sound-pci` to a WAV file). In the same runs: `strace -f -c` writes A's census.json (calls, ioctls, clone, futex and mmap flags, W+X mappings, sockets, signals); a prototype `kai-guest` reads page text over BiDi; image size and boot time are logged. | 720p30 drops at most 1%; 1080p30 at most 5%; guest peak at most 1,280 MiB; cold boot to first frame at most 10 s; warm open of a local page to first frame at most 1 s; WAV non-silent for at least 55 s; 720p30 busy on at most 2 of 4 vCPUs; BiDi read works | Above 5% at 720p: no CPU-rendered route plays well on 4 vCPUs, so the exit-test resolution is rescoped for every route. BiDi fails: page text waits for geckodriver or AT-SPI. |
| S06-3 our hypervisor (2-3 weeks) | A throwaway SVM hypervisor of about 2,000 lines (serial, HLT, MP table, no virtio) boots Debian's stock bzImage and an initramfs from a Limine module, 1 vCPU. Measure (a) boot to init; (b) the `dav1d` CLI decoding a 10 s 1080p AV1 clip, as a ratio against plain KVM; (c) the median of 100,000 port-I/O exit round trips. | (b) at least 0.85; (c) at most 20 µs under nested KVM; (a) at most 5 s under KVM and 180 s under TCG | (b) below 0.6: in-VM video is a P7-only claim. Choose A if S06-2's census passes A's gates (at most 170 distinct calls, at most 60 ioctls, no DRM ioctl, nothing W+X at once). The stock kernel will not boot without ACPI: add minimal ACPI tables before P6. |

Decision rule: choose C if S06-1 to S06-3 pass and the founder answers questions 1 and 3. Otherwise apply the fail column. The spike's SVM code is thrown away; its `cpufeat` bits and CI matrix are kept.

## Phases that depend on it

| Phase | What it needs from this ADR |
|---|---|
| P1 | The rows of item 7 reserved in `agentabi` (enabled when the provider registers in P6); `cpufeat` SVM bits may land early |
| P2 | Large files, the `linux` volume and `package` snapshots (ADR 0004); `Shm` bulk path from `vmm` to `fs` |
| P3 | A foreign `Surface` as a window; input events routed to `vmm` |
| P4 | Nothing required |
| P5 | Verified, chunked download of the layer; NAPT and DNS forward in `netcore`; per-VM network grant |
| P6 | Items 2-13, the exit test; no longer M10b |
| P7 | SVM on the founder's Ryzen 7 7700X; HDA in `audiod` |
| P8 | Fuzzing and review of `vmm` and the VMEXIT path; VMX backend; guest update policy; the licence |
| M10b | Leaves P6's path, but stays before native P3/P4 memory-pressure work; a balloon for the guest comes after it |

## Changes to recently merged and open work

- **#15 seamless actions (merged).** No code change now. P1's registry adds item 7's rows. A name outside the registry is refused (ADR 0002 D2). The guest can never add rows. Why: guest actions then fit the same table.
- **#18 virtio-gpu (open, `origin/dane/virtio-gpu-a5daaa` af7635d).** A follow-up PR, before or just after merge, moves the command constants (kernel/src/display/gpu.rs:28-42) and the request and response layouts into a host-tested `gfxcore::virtio_gpu` module, and adds an EDID writer beside the parser (gfxcore/src/edid.rs:45). It keeps `flush(rect)` (kernel/src/display/mod.rs:141) callable for a surface that is not the screen. Why: `vmm`'s device side mirrors this driver; one tested wire definition cannot drift, and a guest scanout becomes a window.
- **#19 USB input (open, 0717caa).** No change. Keep the HID tablet (xhci/src/hid.rs:209), because virtio-input's tablet takes absolute positions 1:1.
- **#16 DHCP, DNS, ping (open, 131aa77).** No change. Keep the NIC's receive and transmit in one place (kernel/src/net/device.rs) so a later NAPT in `netcore` can carry guest frames. RESOLVE stays 37.
- **#17 terminal look (merged).** No change now. In P6, guest output reaches the terminal as ring-3 output with a `[guest] ` prefix, and ADR 0001's fix to the trusted path covers it: `Kind::of` (termcore/src/style.rs:58-77) returns `Log` for any row that starts with `[`, before the prompt test (`prompt_width`, :112-120), and pieces are cut so ring-3 text never starts a row. There is no separate guest check. Why: text from the guest is data and must never look like the shell's reply, error, question or prompt.
- **#13 parser hardening (merged).** Later, add fuzz targets with `testkit/hostile.rs` for virtio descriptor chains, virtio-gpu commands and kvp/1 frames.
- **#14 decode speed (merged).** None.
- **Docs, in the PR that records approval:** docs/roadmap.md:145 (P6 depends on P1, P2, P3, P5 and virtio-snd, not M10b; exit leg 1 runs in the guest), :146 (audio for the VM moves into P6), :149 (M10b "before native P3/P4 memory-pressure work"); docs/decisions.md:206 becomes "decided by ADR 0006"; :12 and :52 record the 2026-10-20 reuse rule and the 2026-10-21 crate rule, in the same edit ADRs 0007 and 0009 make to those rows.

## Open questions for the founder

1. **Licence.** The guest carries a GPL-2.0 kernel, GPL apt and dpkg, LGPL glibc and Firefox's mixed licences. Should Kernel AI ship the guest image, with its source-offer duties, or should each PC assemble it from Debian's own mirrors at first use? ADR 0011's question 2 and ADR 0009's question 8 ask the same; one answer covers all three. The OS itself stays free of GPL and LGPL either way. This is not legal advice.
2. **Licence: tools and C.** Allow mmdebstrap (MIT) and `mke2fs` (GPL-2), non-crate tools that run only in CI, to build the image, and musl (MIT, C) linked into `kai-guest`, which ships in the image? ADR 0009's question 4 asks the general rule; one answer covers both.
3. **Product.** Does "a static Linux binary runs" mean natively on Kernel AI, or is running inside the bundled guest enough?
4. **Risk.** The in-VM browser is fast only where nested virtualisation works: likely CI under KVM, not VirtualBox on Hyper-V, and on your PC only on real hardware (P7). Is that acceptable for v0.2, or must v0.2 play video in your VirtualBox? If it must, fallback A costs about 20 PM against C's 7-15.
5. **Product.** Is Firefox ESR acceptable as "my browser", or do you mean Chrome? In a Debian guest, Chromium is a package and Google ships Chrome as a `.deb`; neither has been tried in our guest.
6. **Risk and privacy.** Guest updates would come from Debian's mirrors on Debian's schedule, so Debian sees users' IP addresses. Do you accept that, and a standing duty to keep the base layer and `kai-guest` current for as long as Linux apps are offered?
7. **Licence and money.** Should H.264 and AAC be enabled (installing `libavcodec` in the guest), which brings patent questions, or should Kernel AI stay on VP9, AV1 and Opus?
8. **Product.** A browser beside Qwen3-4B needs a VM of about 6.9 GiB (est.), up from the 3,840 MiB that ADR 0010 §4 sets for the 4B from P1. Is 16 GB the minimum PC RAM for the browser?

## Sources

External (read or rechecked on 2026-10-21 unless marked):
- Debian packages, trixie: https://packages.debian.org/trixie/firefox-esr (153.5.1esr-1~deb13u1, installed 301,518 kB; Depends `libvpx9`; Recommends `libavcodec61` or `libavcodec-extra61`), https://packages.debian.org/trixie/cage (0.2.0-2), https://packages.debian.org/trixie/chromium (155.0.8059.79-1~deb13u1, from the security archive), https://packages.debian.org/trixie/linux-image-amd64 (6.12.113-1)
- Debian kernel config, branch debian/6.12/trixie: https://salsa.debian.org/kernel-team/linux/-/raw/debian/6.12/trixie/debian/config/config (virtio options) and `debian/config/kernelarch-x86/config` (`X86_MPPARSE=y`)
- mmdebstrap (README on reproducibility with `SOURCE_DATE_EPOCH`; MIT in the script's header): https://gitlab.mister-muffin.de/josch/mmdebstrap; e2fsprogs copyright (GPL-2): https://metadata.ftp-master.debian.org/changelogs/main/e/e2fsprogs/unstable_copyright
- Firefox remote agent security: https://firefox-source-docs.mozilla.org/remote/Security.html
- Alpine aports `community/firefox-esr/APKBUILD` (140.14.0-r0, release date 2026-08-18, licence field), GitHub mirror, master: https://github.com/alpinelinux/aports
- QEMU `target/i386/cpu.c` `TCG_SVM_FEATURES`: master :963-969, https://gitlab.com/qemu-project/qemu/-/blob/master/target/i386/cpu.c, and v10.2.1 :981-987; virtio-snd: https://github.com/qemu/qemu/blob/master/docs/system/devices/virtio-snd.rst
- VirtualBox manual, nested virtualisation (nested paging not yet passed through to guests) and Hyper-V as its engine: https://www.virtualbox.org/manual/topics/AdvancedTopics.html
- AMD64 Architecture Programmer's Manual vol. 2, pub. 24593, ch. 15 (revision not checked)
- crosvm main ccfda98 (design pass, 2026-10-21): https://chromium.googlesource.com/crosvm/crosvm; Cloud Hypervisor v54.0 (2026-10-14): https://github.com/cloud-hypervisor/cloud-hypervisor; Firecracker v1.17.0 (2026-09-10): https://github.com/firecracker-microvm/firecracker; libkrun v1.19.6 (2026-09-29): https://github.com/containers/libkrun
- Crates on https://crates.io, newest versions on 2026-10-21: iced-x86 1.21.0 (2024-01-20, MIT), ext4-view 1.0.0 (2026-09-14, MIT OR Apache-2.0), linux-loader 0.14.0 (2026-06-26, Apache-2.0 AND BSD-3-Clause), vm-superio 0.8.2 (2026-09-15, Apache-2.0 OR BSD-3-Clause), virtio-queue 0.18.0 (2026-07-07, Apache-2.0 AND BSD-3-Clause), virtio-drivers 0.13.0 (2026-03-03, MIT, with `device::sound`)
- Rust's musl 1.2.5, from Rust 1.93: https://blog.rust-lang.org/2025/12/05/Updating-musl-1.2.5/, https://blog.rust-lang.org/2026/01/22/Rust-1.93.0/
- Sommelier: https://chromium.googlesource.com/chromiumos/platform2/+/HEAD/vm_tools/sommelier/; WSLg: https://github.com/microsoft/wslg
- WSL versions (ms.date 2024-11-19): https://learn.microsoft.com/en-us/windows/wsl/compare-versions; Crostini: https://www.chromium.org/chromium-os/developer-library/guides/containers/containers-and-vms/
- gVisor release-20261012.0 (published 2026-10-16), `test/syscalls/linux`, and its compatibility table ("Of 352 syscalls, 290 ..."): https://github.com/google/gvisor, https://gvisor.dev/docs/user_guide/compatibility/linux/amd64/
- Asterinas v0.19.0 (2026-10-12): https://github.com/asterinas/asterinas; paper: https://arxiv.org/abs/2506.03876
- Ladybird, no public pull requests from 2026-06-05 (post dated that day): https://ladybird.org/posts/changing-how-we-develop-ladybird/; `Meta/CMake/cmake_options.cmake` and `Libraries/LibMedia/DecoderRegistrations.cpp`, master: https://github.com/LadybirdBrowser/ladybird
- Servo MSE issue #22416 (open): https://github.com/servo/servo/issues/22416; mlibc v7.0.0 (2026-08-02): https://github.com/managarm/mlibc; Chromium Ozone: https://chromium.googlesource.com/chromium/src/+/main/docs/ozone_overview.md
- GitHub-hosted runners: https://docs.github.com/en/actions/using-github-hosted-runners/about-github-hosted-runners
- Research-only figures, not rechecked here: WSL1's 235 calls, Asterinas's 210 calls and team size, FreeBSD's 1,661 Chromium patches, Servo's GStreamer licence (work/06/research-prior.md)

Repository (main at ec5ecff unless marked): .github/workflows/ci.yml:48-69; kabi/src/lib.rs:5-7, :101-106, :249; kabi/src/call.rs:4, :6, :125; kabi/src/thread.rs:15; uelf/src/lib.rs:132, :154-157; cpufeat/src/lib.rs:69-92; kernel/src/cpu.rs:42-44, :122-123, :130; kernel/src/fpu.rs:37, :163; kernel/src/memory/user.rs:5-12, :137-138; kernel/src/memory/heap.rs:14; kernel/src/process/loader.rs:133-138; kernel/src/process/futex.rs:15-17; kernel/src/process/mod.rs:54-58; kernel/src/process/workers.rs:158; kernel/src/process/supervision.rs:34; kernel/src/process/burst.rs:25; kernel/src/main.rs:246-248, :481-482; kernel/src/net/device.rs; kernel/Cargo.toml:33; sched/src/lib.rs:39; termcore/src/style.rs:58-77, :112-120; testkit/hostile.rs; xtask/models.json:156; xtask/src/qemu.rs:79-94; xtask/src/vbox/mod.rs:335; docs/decisions.md:12, :18, :39, :52, :123, :171, :174, :177, :182, :203, :206, :211; docs/roadmap.md:131, :145-146, :149, :158. Branches: `origin/dane/virtio-gpu-a5daaa` af7635d (kernel/src/display/gpu.rs:28-42, kernel/src/display/mod.rs:141, gfxcore/src/edid.rs:45); `origin/dane/usb-input-83eb7c` 0717caa (xhci/src/hid.rs:209); `origin/dane/net-dhcp-dns-b5bdb7` 131aa77 (kabi/src/call.rs:134, RESOLVE = 37; the new `netcore` crate). P0 inputs: D:/kai-targets/p0/work/decision-sheet.md and work/06/{judgement,design-A,design-B,design-C,research-code,research-prior}.md; work/04, 05, 07, 08, 11 judgements.
