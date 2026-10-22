# ADR 0008: Updates and recovery

Proposed, 2026-10-21. For the founder's approval; nothing here is decided until approved.

This ADR owns the slot layout of the disk (with [0004](0004-storage.md) for `kai-data`), the boot record, the one TUF repository and root, and the `kaituf` client that [0011](0011-packages.md) also uses. Citations are `path:line` on main at ec5ecff unless a branch is named. Nothing was built or run for it.

## Context

The roadmap asks P0 for "updates and recovery: signed, A/B, with rollback" (docs/roadmap.md:139), P8 for "signed updates with A/B rollback" (:147), and on-disk formats versioned from their first release (:131). Today:

- **Boot.** One Limine 12.9.3 entry (version and SHA-256 pinned at xtask/src/main.rs:61-62) with `timeout: 0`, the whole command line inline and one `module_path` per program (xtask/src/esp.rs:43-51). The kernel ELF is copied unstripped (esp.rs:71); programs are stripped (esp.rs:25). Every program and component reaches the OS as a Limine module (kernel/src/main.rs:466-478), so changing user space means changing the ESP.
- **Image.** xtask writes a deterministic GPT with one ESP entry (xtask/src/image/gpt.rs:62-73) and a one-pass FAT32 (xtask/src/image/fat32.rs, 348 lines). Disk and partition GUIDs come from one fixed seed (gpt.rs:33-40; xtask/src/image/mod.rs:19-20), so two Kernel AI disks in one PC would collide. "The same image is P7's USB stick" (docs/decisions.md:209).
- **Firmware.** OVMF gets a fresh copy of its variable store every boot (xtask/src/main.rs:398-426, attached at xtask/src/qemu.rs:99-106; decisions.md:49) and QEMU runs with `-no-reboot` (qemu.rs:84). Besides the base revision, the kernel makes seven Limine requests, none for EFI (kernel/src/main.rs:94-133), so it has no `SetVariable` and no `ResetSystem`. A panic or `fail` halts with `cli; hlt` outside test mode (main.rs:430-460). `acpitables` reads the MADT, HPET and MCFG but no FADT (acpitables/src/lib.rs:156-166), so there is no reset register. The only watchdog is the scheduler's, in software (decisions.md:61); no hardware watchdog driver exists.
- **Live components.** M9's swap, dual-run probation and kernel ledger work: 16 calls and 30,000 ms by default (livecore/src/settings.rs:29-38), "Time never rolls anything back" (livecore/src/probation.rs:10), a failed version refused without FORCE (livecore/src/ledger.rs:312-318). The ledger lives in kernel RAM. The M9d manifest keeps good, pending and failed marks in host git refs and passes `live_good=`/`live_pending=` on the unauthenticated command line (livecore/src/boot.rs:38-62). A boot that ends in probation tries again with no cap (kernel/src/boot_manifest.rs:7-9; decisions.md:118). Only `notes` is live (livecore/src/boot.rs:13); the shell, llm, supervisor, comphost and edit are native ELF programs (xtask/src/userbuild.rs:12-24).
- **Trust.** Nothing is signed. All six workflows are `contents: read` (for example .github/workflows/ci.yml:8-9), none signs or releases, and actions are pinned by tag, not by commit (ci.yml:24, :29, :137). The only anchor is `xtask/models.json`, which pins Qwen3-4B by repo, revision, size 2,497,281,120 bytes and SHA-256 (models.json:147-156). `build_token=` sits in plain text in `limine.conf` (kernel/src/build_service.rs:47-72). There is no LICENSE file.
- **Versions.** The kernel is 0.10.0 because "0.10 is M10" (kernel/src/version.rs:5-8; kernel/Cargo.toml:3), which SemVer sorts above product v0.1. [0009](0009-licence.md) moves to one workspace version and SemVer tags. Updates here order by `release_seq` and depend on neither.
- **Sizes** (du on the dev PC, 2026-10-21, build profile not recorded): ESPs of 38 and 115 MiB; the unstripped kernel 24,110,064 and 96,473,472 bytes; `kernel.lines` 11,712,937 bytes; VirtualBox's `kai-os.vdi` 39,845,888 bytes.

Since the research read main at 32f2e74 and 2cc927d: PRs #13 (parser hardening), #14 (AVX2 attention), #15 (seamless actions: `shellcore/src/tiers.rs` Run/Ask/Never, `undo.rs` with a RAM journal) and #17 (the "Ember" console, which links Noto Sans Mono into the kernel, kernel/Cargo.toml:35) are merged. #16 (DHCP, DNS, RESOLVE = call 37), #18 (virtio-gpu 2D) and #19 (USB input over xHCI) are green on their branches. A founder rule of 2026-10-21 lets sessions add MIT, Apache-2.0, BSD, ISC and zlib crates without asking, listed in each PR; GPL, LGPL and MPL-2.0 need the founder's OK. The P0 decision sheet names the TUF client `kaituf`, makes it one crate for updates and packages, and puts OS images and packages in one repository.

## Options considered

The judge scored three designs out of 10. The judge's fact-check corrections are applied; the designs' wrong claims are not repeated.

| Design | Score | Evidence for | Evidence against | Verdict |
|---|---|---|---|---|
| **A. A/B image slots** plus a recovery slot R | 8 | R is Limine's default entry. A slot boots only through a one-shot it arms for itself after its own required checks, so a death at any point, even before the kernel can count, lands in R; losing NVRAM also lands in R. The ESP is written once. The OS image never depends on P2's filesystem. One ESP, so the USB stick gets the same layout (UEFI 2.11 §13.3.3: one system partition on removable media). About 3,400 lines to v0.1 and 2,000-3,000 more in P5 (its estimate). | Runtime `SetVariable` from our kernel is unproven. One NV write per boot plus Limine's erase (wear unmeasured). R is one copy. It said little about live components after a reboot. | **Base** |
| **B. Content-addressed generations**: an object store on the ESP and in P2, one menu entry per generation | 5 | One undo list for every system change. Good parts: SHA-256 object ids, chunk lists, an epoch, its action table. | Three object formats, a GC that deletes, and an on-device writer for a FAT ESP with no journal, on the partition firmware needs to boot. Its one-sector `limine.conf` swap is unproven: the name is not 8.3, so it has LFN entries. A failing default has no automatic way out, and `hash_mismatch_panic: no` waits for a keypress, so an unattended machine stops. 0004 already owns `undo`, `update` and `package` snapshots. About 9,700 lines, 6,500 for v0.1, twice A. | Lost; ids, `kai-chunks 1`, the epoch and the rows are kept |
| **C. Component updates first**: live channel, base slots chosen by UEFI `BootNext`, a model channel | 7 | Most updates need no reboot once apps are Wasm ([0003](0003-app-model.md)). The best live-channel durability and model channel of the three; safe mode; an offline update disk. | Per-slot ESPs need two layouts (a USB stick may have one ESP) and `Boot####` entries kept by our kernel against firmware that rewrites `BootOrder`. A committed slot that dies before the kernel counts loops. Three state machines. Only `notes` is live today, so the gain arrives after P3/P4. | Lost as a base; live and model channels, safe mode and the update disk are grafted |

The lever that picks the next boot:

| Lever | Needs | Tries | Verdict |
|---|---|---|---|
| Limine BLI `LoaderEntryOneShot` (A) | runtime `SetVariable`; Limine erases it after reading (bli.c:241-276) and it beats any default (CONFIG.md:112-125) | 1 | **Chosen** |
| UEFI `BootNext` (C) | runtime `SetVariable` and a `Boot####` per slot; used "only on the next boot" (UEFI 2.11 §3) | 1 | P8, with Secure Boot |
| GPT attribute bits (ChromeOS, Bottlerocket) | our own small EFI stage that reads them and chainloads Limine | up to 15 | fallback if S08-1 fails |
| systemd boot-assessment file rename | FAT writes on the ESP; systemd-boot is LGPL-2.1-or-later | per entry | rejected |

Open-source reuse, licences checked 2026-10-21 in the LICENSE file or on crates.io:

| Project, version | Licence | What | Verdict |
|---|---|---|---|
| Limine 12.9.3 | BSD-2-Clause | BLI one-shot, `boot(N)` paths, BLAKE2b path hashes in P8 | already our loader |
| Fuchsia libabr (main) | BSD-style, two clauses (Fuchsia's LICENSE) | the A/B/R record and flow (flow.c, 504 lines) | port to Rust (a ported file: question 6) |
| Bottlerocket signpost `gptprio.rs` (core-kit v17.3.0) | MIT OR Apache-2.0 | GPT priority bits, 95 lines | S08-1 fallback only (question 6) |
| tuf-on-ci 0.21.0 | MIT | the whole signing and publishing service; host only, ships nothing | use (a non-crate tool: question 6) |
| TUF 1.0.37 | specification | roles, thresholds, expiry, root rotation, anti-rollback | follow |
| p256 0.14.0, ecdsa 0.17.0 | Apache-2.0 OR MIT | ECDSA P-256 verification, no_std | use |
| ed25519-compact 2.6.0 | MIT | Ed25519 verification for file keys, no_std | use (also 0011's choice) |
| ed25519-dalek 3.0.0 | BSD-3-Clause | the same, larger, with `verify_strict` | alternative |
| sha2 0.11.1, serde_json 1.0.154 | MIT OR Apache-2.0 | hashes and JSON, both already in the tree (xtask/Cargo.toml:25, kernel/Cargo.toml:31) | use |
| olpc-cjson 0.1.4 | MIT OR Apache-2.0 | canonical JSON, about 500 lines | port only if S08-3 needs it (question 6) |
| tough 0.24.0; rust-tuf 0.3.0-beta9 | MIT OR Apache-2.0 | std clients (tokio, ring); rust-tuf calls itself beta | host test oracle; reference |
| greenboot-rs | BSD-3-Clause | required and wanted health checks | ideas |
| AVB, ChromeOS vboot, update_engine | MIT, BSD-3-Clause, Apache-2.0 | raise the rollback floor only from a good slot; several signatures for key changes | ideas |
| RAUC, OSTree, Nix, systemd-boot, SWUpdate | LGPL-2.1, LGPL-2.0+, LGPL-2.1, LGPL-2.1+, GPL-2.0 | slot backends, atomic switch, generations | ideas only; nothing linked or shipped |
| sbctl 0.18 | MIT | user-owned Secure Boot keys, P8 | use (question 6) |
| virt-firmware 26.10 | GPLv2 | pre-enrolled OVMF variables for P8 tests | host tool only if allowed (question 6) |
| fatfs 0.3.6; blake2 0.11.0 | MIT; MIT OR Apache-2.0 | on-device ESP writes; path hashes; P8 only | use in P8 |

Trust options weighed: signify or minisign keys alone (one key, no expiry, no defence against freeze or mix-and-match); Sigstore or GitHub attestations checked on the device (X.509 and a log proof, and attestations are "not a guarantee that an artifact is secure"); TUF. TUF won: an offline root, threshold keys, expiry and anti-rollback are what P8 needs, and tuf-on-ci already runs the TUF repositories of Sigstore's root signing and GitHub's attestations. The algorithm is forced by the tooling: tuf-on-ci's YubiKey path "currently requires the key type to be eccp256" (docs/YUBIKEY-PIV-SETUP.md:78), securesystemslib's HSM signer supports only P-256 and P-384, and its AWS signer maps no Ed25519. So the device verifies ECDSA P-256.

## Decision

The disk keeps two copies of the system, A and B, and a small recovery copy, R. An update is written whole into the copy that is not running, and the machine restarts into it on trial; if it fails to start or misbehaves, the machine goes back by itself to the copy that worked, or to R. Every update, model and package is signed with keys the founder holds offline, and the machine refuses anything unsigned or older than what it already runs. Apps and live components still update without a restart, as M9's swap does today.

In detail: design A for the disk, the boot lever and trust. Grafts: C's live and model channels, safe mode and update disk; B's object naming, chunk lists, epoch and action rows.

**1. Layout**, the same on an installed disk and on the USB stick:

| # | Name | Size | Holds | Written by |
|---|---|---|---|---|
| 1 | ESP | 64 MiB | Limine 12.9.3 `BOOTX64.EFI`, the constant `limine.conf`, Limine's licence files | xtask or the installer only |
| 2 | `kai-r` | 256 MiB | the recovery slot image | the installer; `updated` only from a release already committed on this machine |
| 3 | `kai-a` | 1 GiB | a slot image | `updated`, only while the slot is inactive |
| 4 | `kai-b` | 1 GiB | a slot image | the same |
| 5 | `kai-data` | the rest | kaifs volumes `system`, `home`, `models` (0004) | `fs` |

- Type GUIDs are our own, from `gpt::guid(label)` (gpt.rs:33-40). Disk and partition GUIDs are unique per install (`xtask image --image-id`, later the installer); CI keeps a fixed seed. Slot images stay byte-for-byte deterministic, which is what makes them signable.
- 2,368 MiB (about 2.3 GiB) is reserved before `kai-data`. A slot image is written at its content size, so a download is the image's size, not 1 GiB.
- The dev loop (`cargo xtask run` with the directory ESP, qemu.rs:144) keeps its one entry.

**2. Slot image.** A deterministic FAT32 image from xtask's writer, written raw:
- `boot/kernel`, stripped.
- `boot/kai.bundle` in `kai-bundle 1`: a header (magic `KAIBNDL1`, version, count), entries {path ≤ 64 bytes, offset, length, SHA-256}, data 4 KiB aligned. It holds `kernel.lines`, `user/*.elf`, `user/*.lines`, `comp/*/*.wasm`, from P4 the built-in apps' signed package repository (0011) and, from P6, 0006's `kai-tools` image, replacing one Limine module per program, so adding a program never changes the config. The kernel checks each SHA-256 (against corruption, not an attacker) and hands the same `(path, bytes)` pairs to `components::init` and `programs::init` as today (main.rs:466-478).
- `boot/slot` in `kai-slot 1`, a release record with one key per line like today's `kai-manifest 1` text (livecore/src/artifact.rs:115). It is not a fourth manifest: apps and packages keep the sheet's three (`kai-package 1`, `kai-app 1`, `kai-manifest 2`). It holds `release_seq` (u32, only rises), `epoch` (u32, rises when a data format changes one way only), `version` (display, SemVer per 0009), `commit`, `channel` (`rc` or `stable`; 0009's `dev` builds are never signed by the real root), `abi` = [0001](0001-capabilities.md)'s `kabi::ABI` (major, minor), the `kai:*` WIT package versions it serves (0003), format versions (`kai-bundle`, `supervisor-state`, kaifs), built-in component labels and `bundle_sha256`.
- `boot/root.json`, the newest TUF root at build time, the device's trust anchor.
- `NOTICES.txt` from 0009, so each update carries its own notices.
- Updates are ordered by `release_seq`, never by SemVer. A device follows `stable` unless the user picks `rc`.

**3. Boot configuration.** `limine.conf` is identical on every install and never rewritten by the OS:

```
timeout: 0
default_entry: 1
/recovery
    protocol: limine
    path: boot(2):/boot/kernel
    cmdline: mode=recovery
    module_path: boot(2):/boot/kai.bundle
/kai-a
    protocol: limine
    path: boot(3):/boot/kernel
    module_path: boot(3):/boot/kai.bundle
/kai-b
    (the same with boot(4))
```

- `default_entry: 1` is written on purpose: then `LoaderEntryDefault` is never read (CONFIG.md:112-125), so no BLI tool or stray variable can pick a slot. No `remember_last_entry`. `boot(N)` is partition N of the boot drive (CONFIG.md:383-391). The BLI ids are `recovery`, `kai-a` and `kai-b` (menu.c:1022-1056).
- The kernel learns its slot from Limine's executable-file response (`ExecutableFileRequest`, limine 0.6.5 src/request.rs:371; `partition_index` and `gpt_disk_uuid`, src/file.rs:24-26), not from the command line.
- No per-machine value is on an installed command line. `build_token=`, `build=` and `llm=` stay dev-loop only. `live.state` (item 6) replaces `live_good=`/`live_pending=`. The model's length, which the kernel needs before `fs` runs, comes from 0004's `kai-modelhint 1` record in block 0 of `kai-data`, rewritten by `fs` after a model commits; this ADR agrees to that carrier. Dev and CI keep `model_len=` (kernel/src/model.rs:180-185).

**4. Boot record and the next-boot rule.** `KaiBootState` is one 64-byte NV+BS+RT EFI variable under a Kernel AI vendor GUID, shaped on libabr's `AbrData`. Bytes: magic `KAB1` (4); version 1.0 (2); for each of R, A and B: priority, tries, successful, reason and failures, a byte each, plus `release_seq` u32 (27); `armed` (1); flags `safe_once` and `recovery_once` (1); `min_release_seq` (4); `min_epoch` (4); `boot_seq` (4); `last_fault` {slot u8, kind u8, code u16} (4); reserved (9); CRC-32 reusing gpt.rs:20-30 (4).

The rule: the one-shot always names the slot to boot next if nothing goes wrong, and no one-shot means R. The events are pure functions in a new host-tested no_std crate, `bootslot`. Its test runs every event sequence up to length 8; each must end on a successful slot or on R, never on a slot with no tries.

| Event | What happens |
|---|---|
| *stage* | Before writing: priority 0, reason Updating, so a half-written slot never boots. After the read-back hash: priority above the active slot, tries 3, successful 0, `release_seq` set. |
| *restart into a slot* | Set `LoaderEntryOneShot` to it and `armed` to it, then reset. |
| *up* (required checks passed) | A trial slot spends one try and arms itself while tries remain. A committed slot arms itself and clears `failures` and `safe_once` if they are set. A normal boot costs one `SetVariable` by us plus Limine's erase. |
| *commit* (probation passed) | successful 1; priority above the other slot; `min_release_seq` and `min_epoch` raised to this slot's. That is AVB's rule: raise the floor only from a good slot. |
| *fault*: a panic, a required service given up (supervise/src/lib.rs:194-202), a failed check, a missed deadline | Write `last_fault`, delete the one-shot, reset. If the firmware call fails inside a panic, the one-shot still names a trial slot, whose tries bound the loop. |
| *R boots* | `armed` names the slot whose boot did not finish. A committed slot gets failures + 1: at 1 R re-arms it; at 2 it re-arms it with `safe_once` (one boot with every component on its built-in version, C's safe mode, through today's fallback path, livecore/src/start.rs:73-88); at 3 it arms the other successful slot. A trial slot is re-armed while it has tries, then marked unbootable, and the other successful slot is armed. Nothing bootable, or a bad CRC: R stays up and interactive. A missing record (NVRAM cleared) is rebuilt from the slots' `boot/slot` files, newest clean `release_seq` first, as a trial. |
| *rollback* (by the user or the agent) | Arm the other successful slot, reset. |
| *recovery on request* | Set `recovery_once`, delete the one-shot, reset. R clears the flag and counts no failure. |

The kernel side is a new `kernel/src/efi.rs`: `EfiRequest` and `EfiMemmapRequest` (limine 0.6.5 src/request.rs:495, :515); one `SetVirtualAddressMap` call to HHDM addresses (from base revision 5 Limine maps the runtime regions and system table into the HHDM, and the kernel asks for revision 6 at main.rs:100); then `GetVariable`, `SetVariable` and `ResetSystem` under one lock, from one kernel thread whose FPU state is saved, because firmware may use SSE inside our soft-float kernel. Outside test mode a panic resets instead of halting; test mode keeps isa-debug-exit. [0007](0007-drivers.md)'s FADT reset register (P1) is the reset before `efi.rs` exists and the fallback after.

**5. Probation, deadline and watchdog.**
- Required checks (greenboot's split): the kernel self-test, the supervisor running, the shell serving; from P2, `kai-data` mounted; from P3, the compositor drawing a frame. Wanted checks only warn: `llm` loaded the model, the network is up. The model is never on the required path; under VirtualBox it once took 116 s to load (decisions.md:211).
- Commit after 20 requests and 10 minutes with no fault, then a last check. An idle machine commits after 4 times that and 2 passing checks. Settings `os_probation_calls` and `os_probation_ms` follow livecore's shape; 600,000 ms is already the most `probation_ms` allows (settings.rs:62). Faults roll back; time never does (decisions.md:91).
- A trial slot that is not up within 300 s faults. The scheduler's 200 ms watchdog (decisions.md:61) and a soft-lockup check on the timer tick take the fault path. A hang with interrupts off needs a hardware watchdog: i6300esb in QEMU, the board's TCO timer in P7 (unverified on the founder's AMD board). A hardware watchdog driver belongs under [0007](0007-drivers.md)'s driver model, which does not list one yet.

**6. Channels**, all signed by the one TUF repository:

| Channel | Unit | Store | TUF role | Applied by | Rolls back to |
|---|---|---|---|---|---|
| base | slot image | `kai-a`, `kai-b` | `targets` | raw write of the inactive slot, read back, restart | the other slot, then R |
| recovery | R image | `kai-r` | `targets` | written only from a release committed here | the USB stick |
| live | Wasm component, ≤ 1 MiB (kabi/src/live.rs:186) | `kai-data` | `targets`; `packages` for apps (0011) | M9's swap and probation, no reboot | the ledger |
| model | GGUF blob | `/system/models/<sha256>.gguf` | delegated `models` | the next boot, on probation | the previous blob |
| packages | `.kpkg` | `/system/pkg/store/<sha256>.kpkg` | delegated `packages` | 0011 | 0011's generations |
| guest (P6) | the `linux-layer` package | the package store | `packages` | [0006](0006-existing-software.md)'s `linux_upgrade`: apt inside the guest after a 0004 `package` snapshot | the snapshot |

- **Live (C).** Release versions get labels `r-<sha12>` (the first 12 hex digits of the artifact's SHA-256; 14 bytes, within the 16-byte limit at kabi/src/live.rs:183-184), beside today's `c-<sha12>` and `e<N>` (decisions.md:98). `updated` installs one through a new call `LIVE_INSTALL(name, bytes, sha256)`; the kernel recomputes the SHA-256 (adding `sha2` to the kernel). `/system/live.state` in `kai-live-state 1` (CBOR; the kernel reads it with livecore's existing parser, livecore/src/cbor.rs:50) holds per component `active`, `good`, `pending`, `pending_boots` (capped at 2, then the version is marked failed), `failed` (≤ 16), `lineage` (6) and the release's `release_seq`. `updated` writes it at every BEGIN, COMMIT and ROLLBACK and hands it to the kernel at boot (`LIVE_MANIFEST`) before any comphost starts. It replaces `live_good=`/`live_pending=` on installed systems; the git-ref path stays for `run --edit`. An interface change is a restart swap (livecore/src/host.rs:243-251).
- **Model (C, with B's chunks).** The `models` role is generated from `xtask/models.json`. Each target carries length, SHA-256, licence (Apache-2.0 or MIT only, decisions.md:84), the Hugging Face URL at the pinned revision (xtask/src/models/catalog.rs:143-145) and a `kai-chunks 1` list: `blob <sha256> <size>`, `chunk 2097152`, then one SHA-256 per 2 MiB chunk (1,191 for Qwen3-4B). A model is signed only after the CI fluency gate on a GitHub runner scores at least what the current model scores (47/50 with today's prompt: GitHub Actions run 39675848260, commit bc0eef3 in PR #10). On the device the switch takes effect at the next boot, because the kernel's model is a `Once` (kernel/src/model.rs:66; 0004) until a later design adds a live switch; `updated` then holds the candidate on probation. It commits when the header passes, health reports the model loaded and 5 fixed M8 smoke tasks pass; it rolls back on a refused header, a failed `MODEL_MAP`, a GiveUp or a failed task. The previous blob stays until the new one has committed and one more boot has completed. Models stay on Hugging Face, pinned by revision and SHA-256, with a GHCR mirror from P5 (0011). CI tests the switch with the generated fixture models; no real model runs on the founder's PC.

**7. Trust.**
- **Repository.** TUF 1.0.37, run by tuf-on-ci 0.21.0 in a new public repository, `dane1599d/kernel-ai-updates`. Metadata is on its GitHub Pages. Slot images, R images and packages are its Release assets, named `<sha256>.<name>` (consistent snapshots). The model cannot be one: a release asset "must be under 2 GiB", and the model is 2.33 GiB. 0009's human-facing release (`.img.zst`, `NOTICES.txt`, the source archive, `SHA256SUMS`) stays on `kernel-ai`'s own Release, and `SHA256SUMS` is also a TUF target.
- **Keys.** Root: 2 of 3 YubiKeys (PIV `eccp256`, firmware 5.7 or later so a later root can add Ed25519), one kept off-site. `targets`, `models` and `packages`: 1 of 2 YubiKeys. `snapshot` and `timestamp`: a cloud KMS key reached through GitHub OIDC, so no private key is a GitHub secret or sits on a runner's disk.
- **Expiry.** Root and targets 365 days; tuf-on-ci starts re-signing root 60 days before expiry (REPOSITORY-MAINTENANCE.md:67, :72). Snapshot 30 days and timestamp 7 days, re-signed daily by a scheduled job; these two figures are our choice, not a tuf-on-ci default.
- **Rotation.** TUF's `N+1.root.json` chain. Each slot carries the newest `root.json`, as signify carries the next release's key in the current one.
- **Device client.** One no_std, host-tested crate, `kaituf`, used by `updated` and 0011's `pkg`: top-level roles, one level of delegation, root rotation, consistent snapshots, expiry and version rules. ECDSA P-256 (`p256` 0.14.0) for YubiKey and KMS keys; Ed25519 (`ed25519-compact` 2.6.0) for file keys, such as the device key that signs locally built apps (0011, [0012](0012-terminal-compiler.md)); SHA-256 (`sha2` 0.11.1). Canonical JSON through serde_json if S08-3 finds it byte-identical, else ported from `olpc-cjson` 0.1.4. `tough` 0.24.0 is the host oracle on shared fixtures; 0011's tuf-conformance run covers the crate too.
- **Clock.** Limine's `DateAtBootRequest` (limine 0.6.5 src/request.rs:544) plus the monotonic clock, never earlier than the newest timestamp seen.
- **Anti-rollback.** A slot below `min_release_seq` or `min_epoch` is refused. The floors rise only after a commit. From P2 the newest root and each role's last version are cached in `/system/trust`, so replayed metadata is refused.

**8. Release.** `.github/workflows/release.yml` (shared with 0009) runs on a `v*` tag in a protected `release` environment. Every action is pinned by commit SHA; only this job gets `contents: write` and `id-token: write`. It builds the slot and R images twice on clean runners and requires one SHA-256, attaches a GitHub artifact attestation (checked by host tooling, not on the device), uploads the assets, and opens the tuf-on-ci signing event for the founder's YubiKey touch; CI then signs snapshot and timestamp and publishes. CI tests use a throwaway repository and keys.

**9. Recovery**, in layers:
1. A component: `:revert`, the failed list and the built-in fallback, as today.
2. Safe mode: one boot on built-in component versions (item 4).
3. The other slot.
4. Slot R: the kernel, supervisor, shell, `fs` (from P2) and `updated`, with no model, so it is the "No AI model is set up" path (decisions.md:144). It draws on the Limine framebuffer through the kernel console, whose font stays in the kernel. It reads PS/2, and USB keyboards through 0007's `usbd` once that ships (P2, after S07-1); the founder's PC needs that in P7. Commands: `:slots`, `:verify` (re-hash both slots), `:rollback`, and `:reinstall` from `/system/updates/` or, from P5, the network.
5. The USB stick, which has the same layout.

**10. Kernel interface.** A new handle kind `Update` (0001) covers the inactive slot, R and the boot variables. It takes the next free kind-ceiling bit in claim order when its PR lands; today's bits 0-8 keep their values (kabi/src/lib.rs:82-108). The kernel grants it only to the service `updated` running the `updated` program, never to apps or scripts. 0001's kind table lists `Update` for P8; this ADR needs it from P2, for the update disk and `:reinstall`. New calls, named here and numbered in claim order after 37 when they land: `SLOT_INFO`, `SLOT_READ`, `SLOT_WRITE` (refused for the active slot, and for R unless the active slot is committed), `BOOTREC_GET`, `BOOTREC_SET`, `LIVE_INSTALL` and `LIVE_MANIFEST`. A restart into a slot arms it and then takes 0007's reboot path (`power_reboot`). Writes use 0004's block write and flush path. Grants live in `/system/grants` on `kai-data`, so they survive a slot switch. Before a new slot's first boot, `updated` asks 0004 for an `update` snapshot of the `system` volume; if that slot fails before commit, the old slot boots on it. `home` is never rolled back by an update.

**11. Agent rows**, declared in [0002](0002-agent-actions.md)'s `agentabi`, owned by `updated` and run with its handles:

| Row | Tier | Undo |
|---|---|---|
| `update_check`, `update_status`, `system_slots` | run | read-only |
| `update_stage` | run | mark the staged slot unbootable |
| `update_restart` | ask; whether a restart your own line asks for still asks once is 0002's question 1, and a restart at a time you set is question 3 here | none for the restart; the slot switch is undone by `system_rollback` |
| `system_rollback` | run | re-arm the slot it left: `Inverse::Slot { release_seq }` |
| `model_use` (a stored model, used from the next restart) | run | the previous model |
| `model_fetch` | ask: a download from a host you did not name (roadmap.md:144; 0002's question 2), unless question 3 here allows it in the background | delete the new blob |

Names follow the sheet's `<group>_<verb>`, so the judge's `slots` becomes `system_slots`. Code writes the result line from `last_fault`, for example "0.3.2 did not start: panic in virtio-gpu; back on 0.3.1".

**12. Later.**
- **P5.** `updated` streams 2 MiB chunks by HTTP Range into the idle slot or the model store, each checked against the signed `kai-chunks 1` list before it is written. A resume starts at the first unchecked chunk; the whole target hash is checked at the end. No deltas at first. DNS goes through RESOLVE; with TUF a spoofed answer can stop an update but never change it, so no DNSSEC is needed.
- **P8 Secure Boot.** One CI-signed Limine and enrolled config per slot under `EFI/KAI/{R,A,B}/`, every path with a BLAKE2b hash (BLAKE2b only: CONFIG.md:436-442), and UEFI `BootNext` as the lever. `KaiBootState` and `bootslot` stay; only `arm(slot)` changes. User-owned keys through sbctl, keeping Microsoft's certificates as sbctl advises; no shim.

Before P5, updates arrive on an update disk, found by the virtio serial `kai-update` as the model disk is found by `kai-model` (decisions.md:208). It holds a copy of the repository, and `updated` checks it exactly as it would a download. `cargo xtask image --stage-b` sideloads slot B for tests.

## Consequences

Good:
- A bad update cannot brick the machine: every failure path ends on a committed slot or on R, and `bootslot`'s exhaustive test checks it.
- An update is a whole image whose bytes are known before it ships, so nothing is ever half applied.
- The OS image does not depend on kaifs (raw partition writes), so 0004 can change without breaking updates.
- The agent can say why a rollback happened, from `last_fault`, and undo an update by name.
- Every format is versioned from its first release: `kai-bundle 1`, `kai-slot 1`, `KAB1` 1.0, `kai-chunks 1`, `kai-live-state 1`.

Costs and limits:
- Every base change is a whole image and a restart (38-115 MiB today, less once stripped). Live components soften this only for Wasm.
- 2.3 GiB is reserved on every install.
- No verified boot before P8: TUF protects the download, not the disk.
- One NV write per normal boot plus Limine's erase; wear is unmeasured.
- R is one copy and lags the slots. A broken R plus a failing slot needs the USB stick.
- It rests on firmware runtime services on every machine. S08-1 settles QEMU and VirtualBox; P7 settles the founder's board.
- P8 writes the ESP again (a signed Limine per slot), so it needs an on-device FAT writer (fatfs, 0004), unless firmware boots EFI files from other partitions (unverified).
- Before P8 no update changes Limine on the ESP; a Limine fix before then is applied from the USB stick or the installer.
- No TPM: the floors live in NVRAM and on disk, which someone with physical access can reset.
- Before P2 there is no version cache: a VM can be fed an older, validly signed timestamp; `release_seq` still refuses older slots.
- Each release needs the founder's YubiKey touch, and the online keys need a KMS account (question 2).
- P-256 is forced by the tooling; its verification time in soft-float ring 3 is unmeasured (S08-3).

## What it costs to change later

| Choice | Cost | Why, and the mitigation |
|---|---|---|
| Partition count and sizes | high | installed disks need a repartitioning migration; so the slots are generous, the type GUIDs are ours, and `kai-slot` carries a layout version |
| TUF and its root | high once devices hold a root | keys rotate by design; leaving TUF needs a signed transition release |
| The lever (one-shot, `BootNext`, GPT bits) | medium | one function, `arm(slot)`, plus P8's ESP layout; the state machine stays |
| FAT32 slots and Limine `boot(N)` paths | medium | tied to filesystems Limine reads; a loader change rewrites the image format, not the record or TUF |
| ECDSA P-256 | low | the scheme is per key; a root rotation adds Ed25519 |
| `KaiBootState` 1.0, `kai-bundle 1`, `kai-slot 1`, `kai-live-state 1`, `kai-chunks 1` | low | versioned; R rebuilds a missing or unknown record |
| Hosting (Pages, Releases, Hugging Face) | low | TUF trusts no host; mirrors are a list of URLs |
| Probation numbers and try counts | low | settings and `bootslot` constants |

## Spikes that settle it

No spike loads a model or runs an LLM, anywhere.

**S08-1 `efi-ab`**, the biggest risk.
- Build, on a branch behind the `spikes` feature (xtask/src/main.rs:307-308, kernel/src/spikes.rs): `efi.rs`, `bootslot`, an R/A/B image with the constant config, fault hooks `panic-early`, `panic-late`, `hang` and `fail-check`, and a QEMU mode that keeps one vars file per VM and allows reboots.
- Run: GitHub Actions, 300 cycles under KVM and 100 under TCG with random faults, plus 50 hard kills of QEMU restarted on the same vars file. On the founder's VirtualBox VM with no model disk: 20 `controlvm reset` cycles and 5 power-off and start cycles. Once, set `LoaderEntryDefault=kai-a` by hand and check that it is ignored.
- Measure: the slot each boot lands on against `bootslot`'s prediction; loops; record CRC failures; NV writes per boot; `SetVariable` latency (TSC); boot time added; the round trip of a failed boot through R. Times are KVM figures; TCG counts landings only (decisions.md:198).
- Pass: 100% predicted landings; every failure sequence reaches a committed slot or R within 4 boots; 0 corrupt records; at most 1 `SetVariable` by us per normal boot; at most 50 ms added per boot under KVM; R round trip at most 15 s under KVM; VirtualBox 20/20 and 5/5.
- Fail: a fault in `SetVirtualAddressMap` or `SetVariable` on OVMF, or a wrong landing, means our own small EFI stage that keeps slot state in GPT attribute bits (`gptprio.rs`) and chainloads Limine; the rest of this ADR stands. VirtualBox alone failing makes it a dev VM without automatic rollback.

**S08-2 slot size and reproducibility**, with 0009's release dry run (S09-1).
- Build the release-profile slot image on two fresh GitHub runners.
- Pass: identical SHA-256, and the image is at most 256 MiB, so 1 GiB slots leave 4 times the room. Above 512 MiB, revisit the slot sizes before any install exists.

**S08-3 `kaituf` primitives**, on one branch with 0011's S11-1 `kaituf-0`; GitHub Actions only.
- Compare serde_json's output with python-tuf's canonical bytes for every metadata file tuf-on-ci writes in a test repository. Pass: byte-identical on 100%; otherwise port `olpc-cjson`.
- Time one P-256 verification in a soft-float ring-3 program. Pass: at most 50 ms under KVM. Under TCG it must stay under 1 s, so boot cases keep their timeouts.

## Phases that depend on it

| Phase | Needs from this ADR |
|---|---|
| P1 | update and model rows declared in `agentabi` with their inverses (and in 0010's prompt budget); `efi.rs` and a panic that resets; `bootslot`; S08-1's result before `KAB1` freezes; the stripped kernel and `kai-bundle 1` |
| P2 | the five-partition layout and `kai-data`; slot writes through `Update`; `KaiBootState` with boot probation; `/system/{live.state,models,trust,updates}`; the `update` snapshot and `kai-modelhint 1` (0004); `updated` with `kaituf` and the update disk |
| P3 | the compositor's frame as a required check; R stays on the kernel console |
| P4 | the model manager on the model channel; a setting for the restart window; built-in apps as signed packages; v0.1 shipped as a signed TUF target (release.yml, `kernel-ai-updates`, the YubiKeys) |
| P5 | chunked HTTPS downloads, `:reinstall` from the network, the GHCR model mirror |
| P6 | `kai-tools` in the bundle; the `linux-layer` package and its snapshots (0006, 0011) |
| P7 | the one-shot and NVRAM measured on the founder's board; a TCO watchdog; R with USB keyboards on that board |
| P8 | an installer that writes the layout with unique GUIDs; Secure Boot per slot; a key-rotation drill; a security review of `kaituf` and `bootslot` |

## Changes to recently merged and open work

| Work | What a follow-up PR changes | Files | Why |
|---|---|---|---|
| #15 seamless actions (merged) | Nothing now. The PR that adds item 11's rows adds `Inverse::Slot { release_seq: u32 }`, keeping the enum `Copy` (0002 names the same variant). | shellcore/src/undo.rs:13-18 | A slot switch lives in NVRAM and survives a reboot; the RAM journal (undo.rs:30) does not. |
| #17 Ember console (merged) | Nothing required; the font stays in the kernel because R's console needs it without user space. A small separate PR now strips the kernel ELF on the ESP with `strip::for_boot`, as programs are, and checks that fault backtraces still resolve. | xtask/src/esp.rs:71 | Slot size, and every release downloads the kernel; `kernel.lines` already carries the line data (esp.rs:19-22). |
| #17 notices | 0009 first copies `NOTICES.txt` to the ESP root; once slots exist (P2) it moves into each slot image, and the ESP keeps only Limine's licence files. | xtask/src/esp.rs | The ESP no longer changes with a release. |
| #13 parser hardening (merged) | Hostile tests in the `testkit/hostile.rs` pattern for `kai-bundle`, `kai-slot`, `KaiBootState`, `kai-chunks`, `kai-live-state` and the TUF JSON reader, as each lands. | new `*/tests/hostile.rs` | Any OS on the machine can write the boot variable; metadata comes from the network. |
| #16 DHCP and DNS (open) | None. `updated` uses RESOLVE (37) under NET from P5. | none | TUF makes DNS spoofing a denial of service at worst. |
| #18 virtio-gpu (open) | None now. When R exists, one boot case runs R with a virtio-gpu attached and checks it draws on the Limine framebuffer. | xtask/src/cases.rs | R must never depend on the GPU driver. |
| #19 USB input (open) | None now. R includes `usbd` once 0007's S07-1 moves xHCI to ring 3 (P2), or keeps the kernel's xHCI driver if S07-1 fails. | R's plan | Otherwise recovery on the founder's PC has no keyboard. |
| #14 decode speed (merged) | None. | none | none |
| PR #10 (merged) | The M8 fluency harness becomes the model channel's release gate. | .github/workflows/fluency-model.yml | A model is signed only if it scores at least the current one. |
| M9d boot manifest | `pending_boots` caps retries at 2; on installed systems the manifest leaves the command line. | kernel/src/boot_manifest.rs:7-9, livecore/src/boot.rs:38-62 | Every A/B system caps tries (ChromeOS at most 15, Fuchsia 7). |
| xtask image | Five partitions, `--image-id`, the constant `limine.conf` for `image` and `vbox`; one kept vars file and no `-no-reboot` for A/B cases. | xtask/src/image/gpt.rs:62-73, image/mod.rs:19-20, esp.rs:43-51, qemu.rs:84, :104-106 | Items 1 and 3. |

On approval, decision rows :49 (a fresh variable store every boot), :118 (the uncapped retry) and :209 (the one-partition boot disk) are marked as amended by this ADR.

## Open questions for the founder

1. **Keys.** Buy three YubiKeys (firmware 5.7 or later) and keep one off-site? Should anyone besides you hold a root key, or do we accept a bus factor of one for now? 0011 relies on the same answer.
2. **Money.** A cloud KMS account for the online keys (AWS, Google Cloud or Azure; a small monthly cost, not priced here): which provider? The alternative is Ed25519 file keys in a protected GitHub environment: no cost, but the key lives in a GitHub secret.
3. **Restarts and downloads.** May Kernel AI restart itself to apply an update at a time you set once, or when idle, or only when you say "restart"? May it download a 2.5 GB model in the background without asking?
4. **Secure Boot.** User-enrolled keys only, which means each user changes a firmware setting, and no Microsoft-signed shim (that needs a legal entity, and review "can reach 2-3 months")? Or plan a company for shim review before v1.0?
5. **Disk.** Reserve about 2.3 GiB on every install for the recovery slot and two system slots?
6. **Code and tools outside the crate rule.** 0009's question 4 sets the general rule; these are the cases. Ported files: libabr's A/B flow (Fuchsia, two-clause BSD-style), and only if a spike needs them, `olpc-cjson` 0.1.4 (MIT OR Apache-2.0, S08-3) or Bottlerocket's `gptprio.rs` (MIT OR Apache-2.0, S08-1). Host-only tools, never shipped: tuf-on-ci 0.21.0 (MIT, Python, runs only in `kernel-ai-updates`'s Actions), sbctl 0.18 (MIT, P8), and virt-firmware 26.10 (GPLv2) for P8's Secure Boot test cases, which as GPL needs your OK.

Listed, not asked (MIT, Apache-2.0 or BSD crates, per the 2026-10-21 rule): `p256` 0.14.0, `ecdsa` 0.17.0, `ed25519-compact` 2.6.0, `sha2` 0.11.1 (new to the kernel and ring 3), `tough` 0.24.0 (host tests only), `blake2` 0.11.0 and `fatfs` 0.3.6 (P8). The version scheme is 0009's. 0011's question on GitHub and Hugging Face seeing users' IP addresses covers updates too.

## Sources

Repo, on main at ec5ecff unless a branch is named:
- Boot and image: xtask/src/esp.rs:19-22, :25, :43-51, :71; xtask/src/image/gpt.rs:20-30, :33-40, :62-73; xtask/src/image/mod.rs:19-20; xtask/src/image/fat32.rs; xtask/src/qemu.rs:84, :99-106, :144; xtask/src/main.rs:61-62, :307-308, :398-426; xtask/src/userbuild.rs:12-24; xtask/Cargo.toml:25; kernel/src/spikes.rs.
- Kernel: kernel/src/main.rs:94-133, :100, :430-460, :466-478; kernel/src/version.rs:5-8; kernel/src/boot_manifest.rs:7-9; kernel/src/build_service.rs:47-72; kernel/src/model.rs:66, :180-185; kernel/Cargo.toml:3, :31, :35; acpitables/src/lib.rs:156-166; kabi/src/lib.rs:82-108; kabi/src/live.rs:183-186; supervise/src/lib.rs:194-202.
- Live editing: livecore/src/settings.rs:29-38, :62; livecore/src/probation.rs:7-10; livecore/src/ledger.rs:312-318; livecore/src/boot.rs:13, :38-62; livecore/src/start.rs:73-88; livecore/src/host.rs:243-251; livecore/src/artifact.rs:115; livecore/src/cbor.rs:50.
- Shell: shellcore/src/undo.rs:13-18, :30; shellcore/src/tiers.rs:35-44; testkit/hostile.rs.
- Models and CI: xtask/models.json:147-156; xtask/src/models/catalog.rs:143-145; .github/workflows/ci.yml:8-9, :24, :29, :137; .github/workflows/fluency-model.yml; commit bc0eef3 (47/50, run 39675848260).
- Docs: docs/roadmap.md:131, :139, :144, :147; docs/decisions.md:49, :61, :84, :91, :98, :118, :144, :198, :208, :209, :211.
- Other ADRs, read 2026-10-21: 0001 (kind table, `Update` in P8), 0002 (`Inverse::Slot`, questions 1 and 2), 0004 (`kai-modelhint 1`, the `update` snapshot), 0006 (`kai-tools`, `linux_upgrade`), 0007 (FADT reboot in P1, `usbd` in P2 after S07-1), 0009 (`NOTICES.txt`, channels, question 4), 0011 (S11-1, built-in apps in the bundle).
- Branches: origin/dane/net-dhcp-dns-b5bdb7 (kabi/src/call.rs, RESOLVE = 37), origin/dane/virtio-gpu-a5daaa (kernel/src/display/mod.rs), origin/dane/usb-input-83eb7c (kernel/src/usb.rs, xhci/src/lib.rs).
- limine crate 0.6.5 (local cargo registry, MIT OR Apache-2.0): src/request.rs:371, :495, :515, :544; src/file.rs:24-26; src/lib.rs:40-45.
- P0 working files, not in the repo: the topic 08 judgement, designs and research, and the P0 decision sheet, 2026-10-21.

External, read 2026-10-21 unless a date is given:
- Limine v12.9.3 (2026-10-06): CONFIG.md :4-12, :87-89, :112-125, :150-153, :383-391, :436-442, https://github.com/Limine-Bootloader/Limine/blob/v12.9.3/CONFIG.md; USAGE.md (Secure Boot, `enroll-config`), https://github.com/Limine-Bootloader/Limine/blob/v12.9.3/USAGE.md; common/lib/bli.c:241-276 and common/menu.c:1022-1056, :1853-1863, :1936, https://github.com/Limine-Bootloader/Limine/tree/v12.9.3/common; COPYING (BSD-2-Clause).
- Limine protocol, base revision 5 (trunk; runtime regions and system table as `RESERVED_MAPPED`): https://github.com/Limine-Bootloader/limine-protocol/blob/trunk/PROTOCOL.md
- UEFI 2.11 §3 Boot Manager: https://uefi.org/specs/UEFI/2.11/03_Boot_Manager.html; §13.3.3: https://uefi.org/specs/UEFI/2.11/13_Protocols_Media_Access.html
- TUF specification 1.0.37 (2026-10-08): https://theupdateframework.github.io/specification/latest/
- tuf-on-ci v0.21.0 (2026-09-15, MIT): https://github.com/theupdateframework/tuf-on-ci; docs/YUBIKEY-PIV-SETUP.md:78, docs/REPOSITORY-MAINTENANCE.md:11-13, :67, :72, docs/ONLINE-SIGNING-SETUP.md.
- securesystemslib (main): `securesystemslib/signer/_hsm_signer.py:56`, `_aws_signer.py:70-80`, https://github.com/secure-systems-lab/securesystemslib
- GitHub: About releases, https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases; deployment environments, https://docs.github.com/en/actions/concepts/workflows-and-actions/deployment-environments; artifact attestations, https://docs.github.com/en/actions/concepts/security/artifact-attestations
- Fuchsia libabr (main; two-clause BSD-style LICENSE; `kAbrMaxTriesRemaining = 7` in data.h): https://fuchsia.googlesource.com/fuchsia/+/main/src/firmware/lib/abr/; OTA (updated 2026-10-12): https://fuchsia.dev/fuchsia-src/concepts/packages/ota
- Bottlerocket `gptprio.rs` (core-kit v17.3.0, 2026-10-21, MIT OR Apache-2.0): https://github.com/bottlerocket-os/bottlerocket-core-kit/blob/develop/sources/updater/signpost/src/gptprio.rs
- ChromeOS cgptlib (BSD-3-Clause): https://chromium.googlesource.com/chromiumos/platform/vboot_reference/+/refs/heads/main/firmware/lib/cgptlib/cgptlib.c; AVB: https://android.googlesource.com/platform/external/avb/+/refs/heads/main/README.md
- systemd Boot Loader Interface and boot assessment: https://systemd.io/BOOT_LOADER_INTERFACE/, https://systemd.io/AUTOMATIC_BOOT_ASSESSMENT/
- greenboot-rs (BSD-3-Clause): https://github.com/fedora-iot/greenboot-rs; RAUC v1.15.2 (2026-03-27, LGPL-2.1): https://rauc.readthedocs.io/en/latest/integration.html; OSTree v2026.4 (2026-08-19, LGPL-2.0+): https://ostreedev.github.io/ostree/atomic-upgrades/
- crates.io, each the newest version on 2026-10-21: p256 0.14.0 (2026-07-03), ecdsa 0.17.0 (2026-07-02), ed25519-compact 2.6.0 (2026-09-25), ed25519-dalek 3.0.0 (2026-07-06), sha2 0.11.1 (2026-10-12), serde_json 1.0.154 (2026-10-11), olpc-cjson 0.1.4 (2024-10-10), tough 0.24.0 (2026-07-10), tuf 0.3.0-beta9 (2022-07-19), blake2 0.11.0 (2026-08-26), fatfs 0.3.6 (2023-01-17): https://crates.io/crates/<name>
- shim-review README ("can reach 2-3 months"): https://github.com/rhboot/shim-review; sbctl 0.18 (2025-10-07, MIT), and its advice to enrol Microsoft's certificates with `-m`: https://github.com/Foxboron/sbctl/blob/master/docs/sbctl.8.txt; virt-firmware 26.10 (2026-10-13, GPLv2): https://pypi.org/project/virt-firmware/
- signify, BSDCan 2015: https://www.openbsd.org/papers/bsdcan-signify.html; YubiKey PIV (Ed25519 from firmware 5.7): https://developers.yubico.com/PIV/Introduction/YubiKey_and_PIV.html
