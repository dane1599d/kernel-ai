# ADR 0004: Storage: filesystem, format versioning, snapshots, crash consistency

Accepted 2026-10-22 (proposed 2026-10-21). The founder approved it with their answers of 2026-10-22 folded in; those points are marked "decided 2026-10-22". Questions still open below keep this ADR's stated default until the founder answers.

Repo citations are `path:line` on main at ec5ecff (origin/main on 2026-10-21) unless a branch is named. External facts were read at the source on 2026-10-21 by this topic's research and judge passes, or by this ADR's check (crates.io API, RedoxFS on GitLab, GitHub releases, Fxfs on googlesource, QEMU source, the VirtualBox manual); anything not checked says "unverified". Nothing was built, run, spiked or measured for this ADR, and no model ran. "Estimate" means judgement, not measurement.

This ADR owns kaifs format 1, its volumes, the `kai-fs 1` protocol, transactions and snapshots, and the `kai-data` partition. [ADR 0008](0008-updates.md) owns the rest of the disk layout.

## Context

Nothing in the guest writes to a disk, and nothing survives a reboot.

- **No write path.** `block::Device` has `read` and no `write` or `flush` (kernel/src/block.rs:12-27). All disks share one lock (:29, :48-57), and a writable disk is logged "writable (used read-only)" (:42). virtio-blk is polled and "Nothing here writes to a disk" (kernel/src/virtio/blk.rs:1-8). NVMe has READ and no WRITE or FLUSH (nvme/src/command.rs:3-8; kernel/src/nvme/mod.rs:1-4). It takes only 512-byte namespaces (kernel/src/nvme/mod.rs:131-137) and gives every namespace its controller's serial (:147). Sectors are 512 bytes everywhere (mm/src/dma.rs:5).
- **A silent flush.** virtio-drivers 0.13.0 has `write_blocks` (src/device/blk.rs:300). Its `flush` returns `Ok(())` without sending a request when FLUSH was not negotiated, and the negotiated set is a private field (src/device/blk.rs:46-51, :141-152).
- **The model is a raw disk.** Before the page budget is set, the kernel copies the whole `kai-model` disk into 2 MiB runs (kernel/src/model.rs:1-17; kernel/src/main.rs:246-248, :482). Its length comes from `model_len=` on the command line (kabi/src/model.rs:79-100). The loader checks only the GGUF magic and version (kernel/src/model.rs:7-8); test boots also check FNV-1a from `model_fnv=` (kernel/src/blk_tests.rs:14-23), "not a defence against a crafted file" (kabi/src/model.rs:114-115). `MODEL` is a `Once`, so a new model needs a reboot (kernel/src/model.rs:66). This was decided as "the raw GGUF file as a read-only disk with no file system" (docs/decisions.md:125).
- **State lives in RAM.** Supervisor state (kabi/src/call.rs:24-26), component checkpoints of at most 64 KiB (kabi/src/live.rs:188), the undo journal of the last run (shellcore/src/undo.rs:29-30) and the last 32 script-log entries (shellcore/src/log.rs:9) are lost at reboot. Settings are command-line words that only xtask writes.
- **Limits a filesystem must fit.** The kernel heap is a fixed 32 MiB (kernel/src/memory/heap.rs:14), so there is no room for a kernel page cache. A process holds at most 512 MiB (kernel/src/process/mod.rs:55-58), and there are 16 process slots (:54). IPC copies at most 8 KiB a message (kabi/src/lib.rs:160; kernel/src/ipc.rs:7-8). There is no munmap or demand paging yet (M10b, docs/roadmap.md:67). There is no entropy source: smoltcp is seeded from the HPET (kernel/src/net/mod.rs:64).
- **Disks in the VMs.** QEMU attaches the model read-only through `-blockdev` (xtask/src/disk.rs:107-129). VirtualBox deletes and rebuilds `kai-os.vdi` on every run (xtask/src/vbox/mod.rs:244-256), and its NVMe controller has two ports (:258-273). xtask writes the GPT and the FAT32 ESP itself (xtask/src/image/gpt.rs:1-4; docs/decisions.md:209).
- **What the roadmap asks.** P0 decides "the filesystem, on-disk format versioning, and copy-on-write snapshots so undo is cheap" (docs/roadmap.md:139). P2's exit: data survives a reboot, a power cut mid-write leaves the filesystem consistent, and `undo` restores a deleted file (:141). On-disk formats are versioned from their first release (:131). Every action is logged and undoable where possible (:130). Storage has been open since M6 (docs/decisions.md:66).
- **Why undo matters most.** The founder wants plain requests to run at once: "if i tell it to do something it should just do it" (docs/decisions.md:205). A change runs without asking only when it has a recorded inverse (shellcore/src/tiers.rs:14; shellcore/src/undo.rs:1-2; [ADR 0002](0002-agent-actions.md)). So the filesystem decides which file actions can just happen.

## Options considered

The judge scored three designs out of 10 on 2026-10-21.

| Design | Score | Evidence | Outcome |
|---|---|---|---|
| **A. Our own copy-on-write filesystem, kaifs**, in a ring-3 `fs` service | 7 | One mechanism, a root pointer per snapshot, gives per-action undo, trash, file history, and rollback for packages. The undo record and the log line land in the same commit as the change, so "done" means undoable on disk. Logic in a host-tested `no_std` crate, as `nvme` and `supervise` are (nvme/src/lib.rs:1-5; supervise/src/lib.rs:1-4). About 10,600 non-test lines, 12-14 PRs, 10-14 weeks (estimate); most of it any design needs (block writes, server, protocol, model path). The highest crash-bug risk of the three. | **Won**, with grafts from B and C. |
| **B. Fork RedoxFS** (MIT, master b87b0976 of 2026-09-07; 0.9.1 of 2026-07-01) | 6 | A 5,560-line `no_std` core with copy-on-write, checksummed pointers and a header ring. But snapshots need a birth generation, and its 16-byte pointer has no room (src/block.rs:258-262), so the format forks on day one and upstream fixes merge by hand. `Disk` has no flush (src/disk/mod.rs:28-54); the header is written right after the batch with no flush between (src/transaction.rs:286-312); the version must match exactly (src/header.rs:63); `//TODO: handle errors` and an `assert` in allocator replay (src/filesystem.rs:161, :283-287). 282 of 449 commits by one author, and no crash testing upstream. | **Lost** as the base; it is the fallback if spike S04-1 fails. Its handle, partition-call, model-fill and flush rules are grafted onto A. |
| **C. ext4 through rsext4 0.9.2** (Apache-2.0, 2026-09-21) **plus a snapshot layer, kaisnap** | 5 | Linux could read the disk. But it needs two undo mechanisms, a snapshot layer with its own format that knows ext4's internals and is invalidated when another OS writes the volume, and it has no data checksums. rsext4 is 41,047 lines, first released 2026-01-28, broke its API in 0.9.0, and is unproven under power cuts. The interop is worth little here: Windows 11 Home reads ext4 only through WSL, and `wsl --mount` needs admin rights and cannot reach USB drives or the Windows boot disk. | **Lost.** ext4-view is kept for reading Linux disks later. |

Reuse options weighed with them:

| Project, version, date | Licence | What it offers | Outcome |
|---|---|---|---|
| RedoxFS `src/allocator.rs`, b87b0976 | MIT ("Copyright (c) 2016 Jeremy Soller") | buddy allocator, about 394 lines; the header-ring idea | **Adapted** into kaifs with its notice (see question 5) |
| Fxfs, Fuchsia main | two-clause BSD text | 75,229 lines in `src/`; needs `std` and `fuchsia_async` | **Method only:** major.minor versions, an earliest supported version (61.0 and 56.0 today), golden images |
| ext4-view 1.0.0, 2026-09-14 | MIT OR Apache-2.0 | read-only ext4, `no_std` | **Later**, to import Linux disks (P6, P7) |
| ext4_rs 1.3.3; rust-fs-ext4 0.8.0 | MIT; MIT | ext4 writers | Rejected: no journal; `std` only |
| lwext4 | GPL-2.0 as a whole | C ext4 | Excluded (GPL) |
| redb 4.4.0, 2026-10-13 | MIT OR Apache-2.0 | embedded database | Not the base: a database, not a snapshot filesystem |
| rust-fatfs 0.3.6, 2023-01-17 | MIT | FAT | **Later**, for exchange sticks |
| Asterinas exFAT and MlsDisk, v0.19.0 | MPL-2.0 | exFAT; an encrypted block layer | Not now; MPL needs the founder's OK |
| littlefs v2.11.3; CrashMonkey and Ace; redb's fuzzer | BSD-3-Clause; Apache-2.0; MIT OR Apache-2.0 | power-loss test methods | **Methods only**, no code |
| OpenZFS 2.4.4; HAMMER2; Btrfs; bcachefs | CDDL-1.0; BSD-3-Clause; GPL-2.0; GPL-2.0 | designs | **References only:** birth times, flush-then-header commits, mark-and-sweep with no reference counts |
| sha2 0.11.1; seahash 4.1.0 (2021-01-16); zerocopy 0.8.63 | MIT OR Apache-2.0; MIT; BSD-2-Clause OR Apache-2.0 OR MIT | checksums; struct parsing without `unsafe` | **Adopted** (one hash, chosen by S04-1) |
| QEMU's blklogwrites driver (since QEMU 3.0; CI runs 8.2, docs/decisions.md:214) | GPL-2.0-or-later | records every guest write and flush | **CI test tool only**, never linked or shipped (question 5) |
| A tar initrd and a RAM filesystem (the old default, docs/decisions.md:66) | ours | nothing persists | Rejected: fails P2's exit |

## Decision

We write our own filesystem, kaifs, built around undo. It never overwrites data in place: a change goes to free space, and one small record written last makes it live, so a power cut leaves either the old state or the new one. Keeping the old version costs almost nothing, so every file change the AI makes can be undone, even after a reboot. It runs as a separate program, `fs`, outside the kernel, and only Kernel AI and its host tool can read the disk.

Base: design A, kaifs. Grafts from B: the flush rule, handles that survive a restart, partition-scoped calls, `MODEL_FILL` with no filesystem parser in ring 0, group commit, the per-node `fmt` byte, `changes` by birth generation, and the shared `diskmap` GPT crate. From C: ext4-view for reading Linux disks later.

### 1. Engine

- `kaifs` is a `no_std` + `alloc` crate, host-tested like `nvme` and `supervise`.
- 4 KiB blocks. One copy-on-write B+tree per volume, with 4 KiB nodes.
- A buddy allocator with levels 0-9, so runs of 4 KiB to 2 MiB are naturally aligned. It is adapted from RedoxFS `src/allocator.rs`, with the MIT notice kept and listed in `xtask/notices.json` (so in `NOTICES.txt`).
- File data sits in records of up to 128 KiB. Sealed blobs use 2 MiB records.
- A `BlockDev` trait (`read`, `write`, `flush`, `info`) runs the same code in `fs`, in xtask over an image file, and in tests over a recording disk. No GPL code.

### 2. Format 1

- **Superblock**, 4 KiB, little-endian, read with zerocopy: magic `KAIFS\0\r\n`; `format_major` and `format_minor` (u32, 1.0 at the first release); `compat`, `ro_compat` and `incompat` (u64 each); uuid, generation, block size and count; roots `volumes`, `alloc` and `gc`, and a `keys` root that stays zero in format 1; commit time; label; `csum_kind`; a checksum of the rest.
- **Mount rules**, ext4's: an unknown incompat bit refuses; an unknown ro_compat bit mounts read-only; an unknown compat bit mounts, but `check --repair` refuses. Another major refuses. The crate records `EARLIEST_SUPPORTED`, as Fxfs does.
- **Superblock rings.** Eight slots at blocks 1-8 and a mirror ring in the last 8 blocks. Block 0 stays outside the filesystem (item 8).
- **`BlockPtr`**, 64 bytes: `addr` u64; `birth` u64, the generation that wrote the block; `blocks` u32, the record length (1-512 blocks); `kind` u8; `level` u8; `flags` u16 (SHARED, SEALED; COMPRESSED and ENCRYPTED reserved); `csum` [u8; 32]; 8 reserved bytes. The checksum sits in the parent, so the tree is a Merkle tree: a torn or misdirected write is caught on read, never returned.
- **`csum_kind`** (an incompat value) is SHA-256 (sha2 0.11.1) or seahash-64 in the 32-byte field. Spikes S04-1 and S04-2 choose it before format 1 is frozen. Provisionally seahash-64: S04-1's portable SHA-256 on the GitHub runner was 268 MB/s, under its 300 MB/s bar ([#84](https://github.com/dane1599d/kernel-ai/pull/84)); final after S04-2.
- **Tree items.** Key `(oid u64, kind u8, off u64)`. INODE: size, mode, nlink, times. DIRENT: `off` is a 64-bit name hash; collisions chain inside the item. DATA: `off` is the file offset and the body a `BlockPtr`; a hole is an absent item. XATTR. Every node carries a `fmt` byte, so an old node is upgraded when it is next written.
- **Volume table.** VOLUME: name, live root, key id. SNAPSHOT: generation, root, kind, expiry, and the log entry that pins it. UNDO: transaction id, volume, the generation before, action id, time, changed paths.
- **Golden images** of format 1 live in host tests from the first release. Every later build must mount them and pass `check`.

### 3. Commit and crash rule

- **Commit N:** write every new record and node to free blocks; FLUSH; write slot N mod 8 in both rings; FLUSH. There is no journal, no replay and no fsck at mount.
- **Mount** reads the 16 slots and takes the newest whose own checksum and whose root's checksum verify.
- **Data** is copy-on-write in the same commit as its metadata. After a crash a file is wholly old or wholly new.
- **Reuse.** A block freed in commit N may be reused from commit N+2, and only if it was born after the newest snapshot of its volume that existed when the block was replaced, and is not SHARED. Every other block waits for the GC. So if slot N+1 tears, commit N still mounts intact.
  - The decision is made when the block is replaced, not at commit. Spike S04-1 ([#84](https://github.com/dane1599d/kernel-ai/pull/84)) found that deciding at commit frees live blocks: snapshot S, commit; change the live tree (its frees wait, since S holds them); restore S; drop S; commit. The waiting frees were then judged against "no snapshot" while S's tree was live. Its regression test is `spikes/kaifs/tests/basic.rs`, `restore_then_drop_keeps_the_restored_blocks`.
  - With two flushes a commit, reuse from N+1 is already safe: commit N is durable before N+1 writes anything, and S04-1's harness finds no violation at N+1. N+2 stays as a margin for anything that overlaps commits, such as group commit.
- **GC.** Mark-and-sweep from live roots, snapshots and open transactions, with no reference counts (HAMMER2's method). It runs in slices of at most 5 ms between requests, and whenever free space is under 10 %.
- **When.** At the end of every action, on `sync`, and at most 1 s after other writes. A mutating call is answered only after its commit is flushed, so "Done" means on disk. All volumes share one superblock, so one commit is atomic across them: a change in `home` and its log line in `system` land together.
- **Devices.** A write-back cache without FLUSH is refused read-write. A device that offers neither FLUSH nor a write-back cache is used as write-through, as VIRTIO 1.3 §5.2.5.1 allows. FUA is not needed. Writes inside one commit go to free blocks, so a driver may issue them in any order and resend them after a reset.

### 4. Transactions, snapshots, undo and trash

- A snapshot is one root pointer in the volume table: constant time, no extra flush.
- Each action is one transaction: `begin(action_id)`, its operations, `commit`. The commit writes the changes, an `undo` snapshot of the volume's previous root and the UNDO record together. A script is one transaction ([ADR 0002](0002-agent-actions.md)); a script that fails half-way changes nothing on disk.
- `restore(txn)` copies the recorded paths back from the snapshot as a new transaction, which can itself be undone. It never overwrites a later change: a path that changed since is restored beside it as `name (restored)`. This is 0002's compare-and-set for files.
- `changes(volume, since)` walks only subtrees whose pointer has `birth > since`, so its cost follows what changed, not the volume's size. It feeds `show` ("what did the AI change?"). `history(path)` lists a file's versions from snapshots.
- Delete moves the item to `/home/.trash/<txn>/` with `info.cbor` `[schema, {path, deleted, action}]`, after the freedesktop Trash 1.0 idea. Undoing a delete moves it back.
- **Retention.** Undo snapshots are pinned by their action-log entry. They expire after 30 days, or earlier, oldest first, when free space is under 10 % (question 3). Emptying the trash and every expiry are logged.
- **Snapshot kinds:** `undo` (0002), `named` (the user's), `update` (0008, before a new slot's first boot), `package` (0011, before a package change; 0006, before a package change inside the Linux guest). ZFS-style dead lists come later behind a compat bit, if S04-1's GC figure asks for them.

### 5. Disk layout and volumes

- GPT as [ADR 0008](0008-updates.md) sets it: ESP 64 MiB, `kai-r` 256 MiB, `kai-a` 1 GiB, `kai-b` 1 GiB, `kai-data` the rest. `kai-data` starts on a 2 MiB boundary. It is found by its type GUID (today's `guid(label)`, xtask/src/image/gpt.rs:33-40) and then by the kaifs uuid, never by serial.
- A shared `no_std` crate, `diskmap`, reads and writes GPT for xtask and the kernel, grown from xtask/src/image/gpt.rs plus a reader.
- This ADR never writes the ESP or the slots.

| Volume | Mounted at | Holds | Undo snapshots |
|---|---|---|---|
| `system` | `/system` | `settings/<area>.cbor` as `[schema, body]` (docs/decisions.md:87); `log` in 0002's `actlog 1`; `grants` in 0001's `kai-grants 1`; `live.state`, `trust/` and `updates/` (0008); `state/<component>/` checkpoints (0003); `pkg/store/<sha256>.kpkg`, `pkg/blobs/` and generations (0011) | yes |
| `home` | `/home` | `/home/user` (one user until P8's accounts); `/home/.trash/<txn>/` | yes |
| `models` | `/system/models` | `<sha256>.gguf`, SEALED, in 2 MiB records; `cache/` for files that can be rebuilt, such as 0010's pack KV file | none |
| `linux` (P6) | `/home/linux` | the Linux guest's writable disks `root.img` and `home.img` (0006), so their snapshots never pin blocks in `home` | none; `package` snapshots only (0006, 0011) |

### 6. Interfaces

- **The kernel block contract**, shared with [ADR 0007](0007-drivers.md), which owns the drivers:
  - `block::Device` gains `write`, `flush` and `info`: volatile cache, flush support, atomic write unit. One lock per disk replaces the one lock over all disks.
  - virtio-blk uses `write_blocks` and `flush`. The kernel reads the device's features before `VirtIOBlk::new`, because the negotiated set is private.
  - NVMe adds WRITE (01h) and FLUSH (00h), and reads VWC (Identify Controller byte 525) and AWUPF.
  - NVMe and virtio-blk stay in ring 0 (0007).
- **Partition handles and calls.**
  - A new handle kind, `Partition`, in [ADR 0001](0001-capabilities.md)'s model. Its kind-ceiling bit is the next free one in claim order.
  - Only the service `fs` running the program `fs` gets startup `Partition` handles, one per `kai-data` partition, with READ and WRITE.
  - New calls `PART_INFO`, `PART_READ`, `PART_WRITE`, `PART_FLUSH` and `MODEL_FILL`. They are numbered in claim order after RESOLVE (37) when the PR lands.
  - The kernel bounds-checks every block. No call reaches the ESP, the slots or a raw model disk.
- **Clients: `kai-fs 1`.**
  - Versioned CBOR requests over today's 8 KiB IPC, at most 7 KiB of data a message.
  - Objects are 0001's `Dir` and `File` kinds: server-backed handles to `fs` whose badge is `(volume, object id)`. The kernel enforces their rights (READ, WRITE, ENUMERATE, CREATE, DELETE); `fs` reads the badge and the right used. Lookup is path-beneath (0001 ports cap-std's method).
  - Operations: `open`, `stat`, `list` (with a cursor), `read`, `write`, `create`, `mkdir`, `rename`, `trash`, `begin`, `commit`, `abort`, `snapshot`, `restore`, `history`, `changes`, `sync`, `df`.
  - A whole-file `write` is atomic by default, so apps cannot get temp-and-rename wrong. ALICE (OSDI '14) found 60 such bugs in 11 applications.
  - Apps reach their own data through [ADR 0003](0003-app-model.md)'s `kai:app` `storage` interface, which `fs` backs with a folder per app. A `kai:fs` WIT mirror of `kai-fs 1` is 0003's to add later.
- **Bulk data.** Copied IPC in v0.1 if S04-2 passes. Otherwise bulk reads and writes use one `Shm` object (0001) with KRNG v1 rings (0007), the same primitive as 0005's surfaces. There is no separate shared-buffer call.
- **Agent rows** (P2). 0002 owns names and tiers; proposed under its naming rule: `fs_list`, `fs_read`, `fs_history`, `fs_changes` (read); `fs_create`, `fs_write`, `fs_move`, `fs_trash`, `fs_restore` (inverse `Snapshot` or `Trash`, so run tier when plainly asked); `fs_empty_trash` (no inverse, so it asks).

### 7. Where it runs

- `fs` is a ring-3 service under supervise, started before `shell`.
- If it dies, the calls it had taken fail at once (kernel/src/ipc.rs:10-14) and its open transaction is gone; nothing half-done is on disk. The restarted `fs` mounts by reading 16 blocks and serves the same badges, since they name objects on disk, not server state.
- A block cache inside `fs`, with CLOCK eviction (mm/src/reclaim.rs:24-27), capped at 64 MiB of its 512 MiB. `fs` takes one of the 16 process slots, and its pages raise `OTHER_SERVICES_PAGES` (kernel/src/model.rs:53).
- No kernel page cache. Mapping files waits for M10b and a kernel pager. P6's micro-VM does not need it: the guest reads its disk files through `vmm` (0006).

### 8. Model store

- **No filesystem parser in ring 0.** The kernel still reserves the 2 MiB runs before the page budget is set.
- `fs` passes the sealed blob's extents, its length and its FNV-1a64 through `MODEL_FILL`. The kernel checks bounds, length, the GGUF header and FNV-1a64, fills the runs and zeroes the tail. Only then does `MODEL_MAP` succeed; before that it returns Busy and `llm` retries.
- SHA-256 of the whole blob is checked once, at import (xtask now) or download (P5, 0008's `kai-chunks 1`).
- **The reservation's size.** Dev and CI keep `model_len=`, which xtask writes. Installed systems have a constant `limine.conf` (0008), so the length cannot come from there. Instead, and 0008 agrees to this carrier: block 0 of `kai-data`, which kaifs leaves out, holds a 512-byte `kai-modelhint 1` record (magic, length of the active model, CRC-32). `fs` rewrites it after a model change commits. The kernel reads that one sector on the disk it booted from. A missing or bad record means no model this boot, and `llm` says so.
- The raw `kai-model` disk stays for dev and CI, so no test case copies a model into an image.
- A new model still needs a reboot (`MODEL` is a `Once`) until a later design adds a live switch.

### 9. Encryption

- Off in v0.1. The guest has no entropy source yet (kernel/src/net/mod.rs:64).
- Format 1 reserves the ENCRYPTED incompat bit, the `keys` root, a key id per volume and the pointer flag.
- P8: AES-256-XTS per volume for `home` and for settings, a keyed MAC in the 32-byte checksum field (XTS alone has no integrity), keys wrapped from a passphrase, and an RDRAND/RDSEED probe in `cpufeat`. `models` stays clear.

### 10. Interop

No ext4 writing. ext4-view 1.0.0 reads Linux disks for import when P6 or P7 needs it. FAT (rust-fatfs) or exFAT serves exchange sticks later.

### 11. Dependencies

All are in the free class under the founder's 2026-10-21 rule, and each PR lists them with their licences: sha2 0.11.1 and zerocopy 0.8.63 (both already in Cargo.lock; xtask/Cargo.toml:25, kernel/Cargo.toml:40); seahash 4.1.0 (MIT), only if S04-1 picks it; later ext4-view 1.0.0 and rust-fatfs 0.3.6; for P8, aes 0.9.3 (MIT OR Apache-2.0), xts-mode 0.6.0 (MIT) and argon2 0.6.0 (MIT OR Apache-2.0) (versions and licences read on crates.io on 2026-10-21). The CBOR coder is question 7.

## Consequences

- Every file change the AI makes has an `undo` that survives a reboot. That lets 0002 run plainly asked create, overwrite, move and delete at once. Only emptying the trash is proposed to ask.
- One mechanism serves file undo (0002), update snapshots (0008), package switches (0011, whose generation switch is one transaction) and the action log.
- Corruption is reported, not returned: every record is checksummed in its parent.
- The format can grow without a reformat: feature bits, major.minor, per-node `fmt`, golden images.
- Costs we accept:
  - New code with the highest crash-bug risk of the three designs. FSCQ, a formally verified filesystem, still shipped a crash bug in its unverified glue (CrashMonkey, OSDI '18). So the crash harness lands with the engine, not after it.
  - No other OS reads kaifs. Recovery needs `cargo xtask fs` on a host; it is plain Rust and runs on Windows without WSL. Exchange with other machines needs FAT or exFAT anyway.
  - Space from expired snapshots comes back only after a GC pass. While an undo snapshot still holds trashed files, emptying the trash frees nothing until that snapshot expires.
  - A 4 KiB random overwrite rewrites a record of up to 128 KiB. This hurts databases and VM disk images in P6; S04-1 measures it.
  - The ring-3 server adds copies through 8 KiB IPC, one process slot and its pages. FUSE costs from nothing to 83 % of throughput (Vangoor et al., FAST '17); ours is S04-2's to measure.
  - SHA-256 runs as portable code in the soft-float kernel and in default-target programs (rust-toolchain.toml:5-6; Cargo.toml:4); only programs built for user/x86_64-kai-user.json have SSE (user/x86_64-kai-user.json:8). Its speed here is unmeasured.
  - Snapshots are not backups, as the Btrfs documentation warns. One disk has no mirror.
- **Effort** (estimate): about 11,000 non-test lines (design A's 10,600 plus B's `diskmap`) in 12-14 PRs, 10-14 weeks of one stream. Order of the P2 PRs, which [0000-execution-plan.md](0000-execution-plan.md) schedules:

| # | PR | Size |
|---|---|---|
| 1 | Block write and flush in `block`, virtio-blk and NVMe; per-disk locks; boot case write, reboot, read back; S04-3's blklogwrites case | ~400 |
| 2 | `diskmap`; `kai-data` disks for QEMU and VirtualBox | ~500 |
| 3 | `Partition` kind, `PART_*` calls, an `fs` plan line with a stub server; S04-2 | ~300 |
| 4-5 | kaifs core from S04-1: format, tree, allocator, commit, mount, crash enumerator | ~3,500 + ~2,000 tests |
| 6 | Namespace: inodes, directories, records, rename, trash, `check`, golden image, hostile-input tests | ~2,000 |
| 7 | Volumes, snapshots, UNDO, `restore`, `changes`, GC | ~1,200 |
| 8 | The `fs` server, `kai-fs 1`, `Dir`/`File` badges, cache, restart case, `fs_*` rows | ~1,200 |
| 9 | xtask `fs mkfs`, `import`, `ls`, `cat` and `check`; the blklogwrites log reader | ~900 |
| 10 | Model blobs, `MODEL_FILL`, `kai-modelhint 1` | ~500 |
| 11 | Settings, grants and the action log on disk; undo survives a reboot | ~600 |

## What it costs to change later

| Change | Cost |
|---|---|
| Anything, before v0.1 ships | Almost nothing: test VMs are re-imaged and `xtask fs mkfs` rebuilds data disks. |
| New features after v0.1 | Low: a compat or ro_compat bit; the golden images catch accidents. |
| `BlockPtr` layout, node size, checksum algorithm | Highest: an incompat bit plus rewriting nodes as they are next written, or a copy migration. Hence S04-1 and S04-2 choose them first, and format 1 keeps 8 spare bytes a pointer and reserved superblock space. |
| Swapping the engine (the RedoxFS fork, or another filesystem) | Medium: `kai-fs 1`, `restore(txn)`, tiers and actions stay; data is copied across; old versions survive only if the new engine has snapshots. |
| Moving `fs` into the kernel | Low-medium: the crate is `no_std`, so about 1,000 lines of glue change (estimate); the cache would need frames, not the fixed heap. |
| Moving block drivers to ring 3 (0007) | Low: only the `BlockDev` adaptor changes. |
| Copied IPC to `Shm` bulk transfers | Low: additive, a `kai-fs 1` minor version. |
| Turning on encryption | Medium: the format is ready; existing data is rewritten per volume. |
| `kai-modelhint 1`, if a later design moves the model length elsewhere | Low: one sector and two small readers. |

## Spikes that settle it

No spike runs a real model, and none runs inference on the founder's PC.

**S04-1, kaifs crash core** (design A's spike, extended). Host-only crate `spikes/kaifs`, beside spikes/rhai and spikes/wasmi (Cargo.toml:3). Ten working days. Runs with `cargo test` in GitHub Actions on ubuntu-24.04, and locally; no VM, no model.

- **Build:** superblock rings, `BlockPtr`, a copy-on-write B+tree of u64 keys with values of at most 1 KiB, the RedoxFS-derived allocator, commit, snapshots, the N+2 rule, mark-and-sweep and `check()`. A `RecordingDisk` that logs writes and flushes. A crash enumerator: every prefix; every subset of the open flush epoch up to 12 writes, and 4,096 random subsets above that; superblock slots torn at 512-byte steps.
- **Workloads:** every sequence of up to 3 operations from {put, delete, overwrite, snapshot, drop snapshot, restore, commit}, on an empty tree, a 1,000-key tree and a 100,000-key tree.
- **Oracle:** mount succeeds, `check()` is clean, and the state equals one committed generation no older than the last commit that was waited on.

| Measure | Pass | Fail |
|---|---|---|
| Violations over at least 1,000,000 crash states | 0 | any left unfixed at the end of the time box |
| Non-test lines for the spike's scope | ≤ 3,500 | > 5,000 |
| Bytes written per byte changed: 4 KiB random overwrites, 64 per commit, 100,000 keys | ≤ 3.0x | > 6.0x |
| Cost of a snapshot | 0 extra flushes, ≤ 1 extra block | more |
| GC mark: 100,000 keys, 100 snapshots 64 random operations apart | < 2 s | > 10 s: dead lists move into format 1 |
| SHA-256 against seahash-64, per 128 KiB record, portable code, host | SHA-256 ≥ 300 MB/s: SHA-256 | below: seahash-64 in the 32-byte field |

**If S04-1 fails** on violations or size, run design B's 5-day spike: vendor RedoxFS b87b0976 unchanged with `default-features = false`, add only `Disk::flush` and the two flushes, and run the same harness. It passes if it builds for both targets with at most 150 changed lines, and shows zero failing crash states, or failures fixed with at most 300 changed lines in transaction.rs and allocator.rs and no new commit protocol. Then B's format fork replaces A's engine. Everything above the engine stays.

**S04-2, `fs` over IPC.** GitHub Actions, QEMU under KVM, no model. A ring-3 stub `fs` answers 10,000 calls of 8 KiB, and a 64 MiB file is written and read through it.
- **Pass:** at least 50 MB/s each way; v0.1 keeps copied IPC.
- **Fail:** below that, bulk data moves to `Shm` with KRNG v1 rings in P2.
- Also measure SHA-256 inside the `fs` build target. If it is under a third of S04-1's host figure, `csum_kind` becomes seahash-64 before format 1 is frozen.

**S04-3, flush honoured.** It lands with P2's first PR as a raw block-write case, and gains file actions once `fs` serves them. It must pass before any user data is stored.
- **QEMU** (GitHub Actions under KVM and TCG; locally under TCG in WSL2): `kai-data` behind blklogwrites, with the log on a second file, the default writeback cache, and never `-snapshot` (which makes caching unsafe). The guest runs 200 file actions. The host replays the log to every flush and FUA point and runs `xtask fs check` on each. Pass: 0 violations. Also report commit latency (p50, p99; two flushes each) for 0002's typing-to-done table.
- **VirtualBox** (the founder's PC, storage only, no model): `xtask vbox` powers the VM off hard 20 times mid-write, then mounts and checks. Pass: 20/20 clean, with the last waited-on commit present. VirtualBox ignores guest flushes on IDE and SATA by default (its manual, "Responding to Guest IDE/SATA Flush Requests", gives an `IgnoreFlush` key); whether NVMe behaves the same, and whether an NVMe key exists, is unverified. On failure, set such a key if it exists, or document VirtualBox as not power-safe. A VM power-off is not a host power cut, so this is a smoke test; the QEMU replay is the proof.

## Phases that depend on it

| Phase | What it needs from this ADR |
|---|---|
| P1 | Undo and log shapes that map onto `restore(txn)` (0002); the log stays in memory until P2. S04-1 can run now: it is host-only. |
| P2 | All of it: block writes, `Partition` and its calls, kaifs, `fs`, `kai-fs 1`, volumes, snapshots, trash, settings, grants and log on disk, model blobs, S04-2 and S04-3. Also 0011's store and generations, 0010's pack KV file (rebuildable, so it sits in `models`' `cache/` with no undo snapshots), 0012's `#include` files and saved sessions. |
| P3 | Fonts, themes and keymaps as files (0005); `Shm` shared with surfaces if S04-2 failed. |
| P4 | The files, notes, settings and model manager apps; editor drafts that survive a crash; per-app storage (0003); `c>` sessions (0012). |
| P5 | Verified downloads with resume into `/system/models` and the package store, in 2 MiB chunks (0008, 0011); `kaituf`'s cached TUF metadata in `/system/trust` (0008). |
| P6 | The micro-VM path (0006): the `linux` volume with `root.img` and `home.img` as large sparse files, copied from the sealed `linux-layer` blob in `/system/pkg/blobs/` (0011) at first start and on "reset Linux", with a `package` snapshot before each guest package change; the `Shm` bulk path from `vmm` to `fs`; smaller records for VM disk files, which the pointer's `blocks` field already allows. POSIX mode, nlink and times (in format 1) and file mapping through a pager matter only if 0006's fallback `lxabi` is taken. |
| P7 | NVMe writes on real disks with VWC and AWUPF; 4Kn namespaces (the 4 KiB block fits; the driver change is 0007's); disks behind PCI bridges (0007); hostile images from USB sticks; ext4-view import; FAT or exFAT exchange. |
| P8 | The installer writes the GPT and formats `kai-data` (0008); encryption; accounts; image fuzzing; a security review. |

## Changes to recently merged and open work

No follow-up PR is needed now. Exactly what later PRs change:

| Work | State | Change | Files | Why |
|---|---|---|---|---|
| #15 seamless actions | merged | None now. In P2 the file actions become 0002 registry rows with inverse `Snapshot` or `Trash`, not new `TIERS` rows. `Inverse` gains `Fs { txn: u64 }` and stays `Copy`; `revert` keeps going through `Host` (undo.rs:48), so a restore is one more `Host` method. The in-memory log (log.rs:9) becomes 0002's `actlog 1` in `/system/log`. | shellcore/src/undo.rs, shellcore/src/log.rs | `fs` keeps the paths, so the shell's inverse stays small and durable. |
| #13 parser hardening | merged | None now. Each new parser gets `testkit/hostile.rs` tests in its first PR: `diskmap`'s GPT reader, the superblock, node and item readers, the `kai-fs 1` decoder, `info.cbor`, `kai-modelhint 1`. | new `tests/hostile.rs` files | Disks become untrusted input in P7. |
| #17 Ember theme | merged | None. The theme stays built in, which 0008's recovery console needs. A user theme setting, if one is added, is a file under `/system/settings`. | none | |
| #14 AVX2 attention | merged | None. | none | |
| #16 DHCP, DNS, ping | open, green | None. RESOLVE keeps 37, behind `cap::NET` on the branch (kabi/src/call.rs:126-134) and later behind 0001's `cap::RESOLVE` bit; storage's calls and its `Partition` bit come after it when they land. Moving network settings from the command line (kernel/src/net/setup.rs:1-2, :34-35 on the branch) to `/system/settings/net.cbor` is a later PR. | none | |
| #18 virtio-gpu 2D | open, green | None. Merge P2's block-write PR after it: both edit kernel/src/virtio/mod.rs (#19 does too), and #18 adds a `cmdline` argument to `virtio::init`. | none | Avoids rebase churn. |
| #19 USB input | open, green | None. Merge P2's data-disk PR after it: both edit xtask/src/vbox/mod.rs and xtask/src/qemu.rs. xhci has no bulk transfers (xhci/src/lib.rs:12-14 on the branch), so no USB mass storage; its `Hal` seam is the model for `BlockDev`. | none | |
| PR #6 code (block layer, VirtualBox) | merged earlier | In P2: the "used read-only" log line goes (kernel/src/block.rs:42); the NVMe controller gets a third port for `kai-data.vdi` (xtask/src/vbox/mod.rs:258-273), created once and never deleted with the OS disk (:244-256). | those files | The data disk must outlive OS rebuilds. |
| Docs | main | On approval: a decisions.md row that closes storage (open since :66); :125 amended (the raw model disk stays for dev and CI; installed systems use `/system/models`); the RedoxFS allocator waits for 0009's reuse row over :30 and :52 ("clean-room"); roadmap.md:141's "move and overwrite ask, with undo" becomes "run when plainly asked, with undo", following decisions.md:205 and 0002. | docs/decisions.md, docs/roadmap.md | |

## Open questions for the founder

1. **Disk format.** Your data disk would be readable only by Kernel AI and its host tool (`cargo xtask fs`), not by Linux or Windows. In return every AI file action gets snapshots and undo. Is that the right trade, or do you want Linux-readable disks (design C) with weaker undo?
2. **Dual boot.** Do you plan to dual-boot Linux beside Kernel AI on your PC in P7? If yes, ext4 import matters more.
3. **Undo history.** Keep 30 days, and drop the oldest when the disk is under 10 % free, without asking? Emptying the trash would be the one file action that asks (0002's question 1 too).
4. **Encryption at rest.** Is P8 (v1.0), with a passphrase at boot, soon enough, or must v0.1 have it?
5. **Partly decided 2026-10-22:** GPL tools (QEMU's blklogwrites, e2fsprogs) may run in CI, never shipped. The copied RedoxFS allocator (MIT source, not a crate) waits on ADR 0009's question 4.
6. **Risk.** v0.1's filesystem would be new code, proven by crash tests in CI and in VMs only, with real disks from P7. Is that acceptable for a VM release?
7. **Decided 2026-10-22:** BlueOak-1.0.0 is in the free class, so `kai-fs 1`, settings and `info.cbor` use minicbor 2.3.0.

## Sources

Repo, on main at ec5ecff unless a branch is named:
- kernel/src/block.rs:12-57; kernel/src/virtio/blk.rs:1-8; nvme/src/command.rs:3-8; kernel/src/nvme/mod.rs:1-4, :131-137, :147; mm/src/dma.rs:5; mm/src/reclaim.rs:24-27.
- kernel/src/model.rs:1-17, :53, :66; kernel/src/main.rs:246-248, :482; kernel/src/blk_tests.rs:14-23; kabi/src/model.rs:79-100, :114-115; xtask/models.json:155-156.
- kabi/src/lib.rs:81-109, :160; kabi/src/call.rs:24-26, :125; kabi/src/live.rs:188; kernel/src/ipc.rs:7-14; kernel/src/process/mod.rs:54-58; kernel/src/memory/heap.rs:14; kernel/src/net/mod.rs:64.
- shellcore/src/undo.rs:1-55; shellcore/src/tiers.rs:13-44; shellcore/src/log.rs:9; livecore/src/state.rs:1-3; livecore/src/cbor.rs; testkit/hostile.rs; supervise/src/lib.rs:1-4.
- xtask/src/disk.rs:107-129; xtask/src/vbox/mod.rs:244-273; xtask/src/image/gpt.rs:1-4, :33-40; xtask/Cargo.toml:25; kernel/Cargo.toml:40; Cargo.toml:3-4; Cargo.lock (sha2 0.11.1, zerocopy 0.8.63, minicbor 2.3.0); rust-toolchain.toml:5-6; user/Cargo.toml:10-11; user/x86_64-kai-user.json:8.
- docs/decisions.md:30, :52, :66, :87, :124, :125, :205, :209, :214; docs/roadmap.md:67, :102, :130-131, :139-147.
- Branches: origin/dane/net-dhcp-dns-b5bdb7 (131aa77) kabi/src/call.rs:126-134, kernel/src/net/setup.rs:1-2, :34-35; origin/dane/virtio-gpu-a5daaa (af7635d) kernel/src/virtio/mod.rs:16; origin/dane/usb-input-83eb7c (0717caa) xhci/src/lib.rs:12-14, kernel/src/virtio/mod.rs, xtask/src/vbox/mod.rs, xtask/src/qemu.rs.
- Other ADRs: 0001 D2 (`Dir`, `File`, `Partition` kinds) and D3 (ceiling bits); 0006's `linux` volume; 0008's layout, `/system/trust` and agreement to `kai-modelhint 1`; 0010's pack KV file.
- Inputs: D:/kai-targets/p0/work/decision-sheet.md; D:/kai-targets/p0/work/04/{judgement,design-A,design-B,design-C,research-code,research-prior}.md.

External (read 2026-10-21 unless dated otherwise):
- VIRTIO 1.3 csd01, §5.2.5.1: https://docs.oasis-open.org/virtio/virtio/v1.3/csd01/virtio-v1.3-csd01.html
- virtio-drivers 0.13.0 (2026-03-03, MIT), src/device/blk.rs:14-18, :46-51, :141-152, :300: https://crates.io/crates/virtio-drivers
- NVM Express Base Specification 2.0 (the source of the `nvme` crate); opcodes and Identify offsets cross-checked against Linux include/linux/nvme.h (reference only).
- VirtualBox manual, "Responding to Guest IDE/SATA Flush Requests": https://www.virtualbox.org/manual/topics/Troubleshooting.html
- QEMU invocation (master): https://www.qemu.org/docs/master/system/invocation.html ; blklogwrites on QEMU master, read 2026-10-21: block/blklogwrites.c ("GNU GPL, version 2 or later"), qapi/block-core.json:3996 ("Since: 3.0"); dm-log-writes: https://docs.kernel.org/admin-guide/device-mapper/log-writes.html
- ext4 superblock and feature rules (page 7.3.0): https://docs.kernel.org/filesystems/ext4/super.html
- Ahrens, "How ZFS snapshots really work", BSDCan 2019: https://papers.freebsd.org/2019/bsdcan/ahrens-how_zfs_snapshots_really_work/
- HAMMER2 DESIGN v6 (2018-12-08): https://github.com/DragonFlyBSD/DragonFlyBSD/blob/master/sys/vfs/hammer2/DESIGN
- Fxfs RFC-0136 (2021-10-13): https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0136_fxfs ; src/serialized_types/types.rs on main, read 2026-10-21 (LATEST_VERSION 61.0 at :44; EARLIEST_SUPPORTED_VERSION 56.0 at :58): https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/storage/fxfs/
- RedoxFS master b87b0976 (2026-09-07; LICENSE "The MIT License (MIT)"; src/allocator.rs 394 lines; `BlockPtr` at src/block.rs:258-262; 449 commits, 282 by Jeremy Soller under two names), 0.9.1 (2026-07-01; crates.io shows its licence as "non-standard", meaning a licence file): https://gitlab.redox-os.org/redox-os/redoxfs
- rsext4 0.9.2 (2026-09-21, Apache-2.0; crates.io points at Starry-OS/rsext4, published commit bd0153f7 in rcore-os/tgoskits `fs/rsext4`): https://crates.io/crates/rsext4
- ext4-view 1.0.0 (2026-09-14): https://crates.io/crates/ext4-view ; rust-fatfs 0.3.6 (2023-01-17): https://crates.io/crates/fatfs ; redb 4.4.0 (2026-10-13): https://github.com/cberner/redb ; lwext4: https://github.com/gkostka/lwext4 ; littlefs v2.11.3 (2026-03-25, BSD-3-Clause): https://github.com/littlefs-project/littlefs ; Asterinas v0.19.0 (2026-10-12, MPL-2.0): https://github.com/asterinas/asterinas ; OpenZFS 2.4.4 (2026-08-21): https://github.com/openzfs/zfs
- crates.io API, 2026-10-21: sha2 0.11.1 (2026-10-12), zerocopy 0.8.63 (2026-10-13), seahash 4.1.0 (2021-01-16, MIT), aes 0.9.3 (2026-08-28), xts-mode 0.6.0 (2026-05-05, MIT), argon2 0.6.0 (2026-08-27), ext4_rs 1.3.3 (2026-01-13, MIT), rust-fs-ext4 0.8.0 (2026-10-06, MIT), rsext4 (first release 2026-01-28, 26 versions), minicbor 2.3.0 in Cargo.lock (2.3.1 released 2026-10-18).
- CrashMonkey and Ace, OSDI '18: https://www.usenix.org/conference/osdi18/presentation/mohan ; ALICE, OSDI '14: https://www.usenix.org/conference/osdi14/technical-sessions/presentation/pillai ; Vangoor, Tarasov and Zadok, FAST '17: https://www.usenix.org/conference/fast17/technical-sessions/presentation/vangoor
- Btrfs subvolumes: https://btrfs.readthedocs.io/en/latest/Subvolumes.html ; freedesktop Trash 1.0: https://specifications.freedesktop.org/trash/1.0
- `wsl --mount` (ms.date 2023-07-17): https://learn.microsoft.com/windows/wsl/wsl2-mount-disk
