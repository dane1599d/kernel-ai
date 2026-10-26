# P2 plan: files and storage

Status: DRAFT for founder approval, 2026-10-26. No P2 code lands on main until the founder approves this plan; spikes S04-2 and S04-3 run on `spike/*` branches meanwhile. It plans the work that the accepted ADRs (`docs/adr/`, accepted 2026-10-22) and the execution plan ([ADR 0000](../adr/0000-execution-plan.md)) put in P2. Section 4 lists the few things it decides itself, because no ADR covers them. Nothing here was built or run for this plan; sizes marked "est." are estimates.

## 1. Goal and exit

**Goal** (roadmap 3b, P2): a writable virtio-blk disk (NVMe on real hardware in P7), [ADR 0004](../adr/0004-storage.md)'s filesystem kaifs with snapshots, a home directory, and settings and the model store on disk. File actions: list, read and create run at once; move and overwrite ask, with undo; delete goes through a trash.

What the founder sees at the end of P2, at the `kai>` prompt:

- **Files survive a reboot.** "make a file called plan.txt saying buy milk" runs at once and answers `Done: created plan.txt (8 bytes). undo`. After a reboot, "what's in plan.txt" reads it back.
- **Moves and overwrites ask, and can be undone.** "rename plan.txt to todo.txt" shows the Allow / Deny menu. After Allow, `undo` puts the old name back.
- **Delete goes to the trash.** "delete todo.txt" runs, answers `Done: moved todo.txt to the trash. undo`, and `undo` brings it back, even after a reboot.
- **Emptying the trash asks**, because nothing can undo it.
- **"What did the AI change today?"** lists the changed files (`fs_changes`), and `fs_history` lists a file's earlier versions.
- **Settings, standing rules and the action log outlive a reboot.** An "Always allow" rule is still there after a reboot, and so is `:log`.
- **The model loads from the data disk.** With a model imported, `llm` maps `/system/models/<sha256>.gguf` with no separate `kai-model` disk.

**Exit criteria.** All of these pass in CI, under KVM and TCG, with the mock model keyed by request. The roadmap's three come first:

| Boot case | Shows |
|---|---|
| `fs-reboot` | Data survives a reboot: files written in the first boot read back byte for byte in a second boot on the same data disk (roadmap 3b) |
| `fs-powercut`, with S04-3's replay | A power cut mid-write leaves the filesystem consistent: QEMU's blklogwrites log is replayed to every flush and FUA point and `cargo xtask fs check` passes on each, with the last waited-on commit present; 20 hard kills of QEMU mid-write each mount clean on the next boot (roadmap 3b) |
| `fs-undo` | `undo` restores a deleted file from the trash (roadmap 3b) |

This plan adds cases for the rest of the deliverable:

| Boot case | Shows |
|---|---|
| `blk-write` | Write, flush, reboot and read back, on virtio-blk and on NVMe |
| `fs-act` | `fs_list`, `fs_read` and `fs_create` run with no prompt; `fs_move`, and `fs_write` over an existing file, ask; Deny changes nothing; `undo` after Allow restores; a restore over a later change lands beside it as `name (restored)` |
| `fs-undo-reboot` | `undo` restores a deleted file after a reboot; a standing rule and a setting are still there |
| `fs-restart` | `fs` killed mid-transaction restarts; nothing half-done is on disk; open `File` handles work again |
| `model-store` | `llm` maps a model that MODEL_FILL filled from `/system/models`; a blob whose FNV-1a64 is wrong is refused, and `llm` says so |
| `pkg-store` | An install survives a reboot; `undo` puts the previous generation back after a reboot |

Host gates in CI:

- kaifs: at least 1,000,000 crash states with 0 violations, S04-1's harness now run over kaifs proper and its namespace operations;
- the golden images of format 1 mount and pass `check` on every build;
- hostile tests through `testkit/hostile.rs` for every new parser: `diskmap`'s GPT reader, the superblock, node and item readers, the `kai-fs 1` decoder, `info.cbor`, `kai-modelhint 1` and `kai-gen 1`;
- S11-2's store harness: 0 invariant violations.

No P2 gate needs the real model, so P2 adds no real-model runner jobs.

## 2. Entry, and what comes before P2's first PR

**Entry conditions** ([ADR 0000](../adr/0000-execution-plan.md) section 3), all met:

- P1's handle steps 1-3 are merged (decisions rows of 2026-10-22 and 2026-10-23: handles, routes and handle IPC, grants and ONCE).
- Spike S04-1 passed (draft PR #84): 0 violations over 1,335,866 crash states, 1,561 core lines, 2.96x on 4 KiB overwrites, free snapshots, a 0.06 s GC.
- Batch 2 (ADRs 0004, 0007, 0008, 0011 and the rest of 0009) was accepted on 2026-10-22.

**Before the first P2 code PR:**

1. **This plan is approved.**
2. **#88 merges.** It rewrites ADR 0004's reuse rule after S04-1 (a block's reuse is decided when it is replaced, not at commit) and sets `csum_kind` to seahash-64, provisionally. Task 4 builds on that wording.
3. **S04-2 reports** (`spike/fs-ipc`). Its result decides whether tasks 3 and 8 move bulk data over copied IPC or over `Shm` with KRNG v1 rings.
4. **S04-3's raw half reports** (`spike/blk-flush`). Task 1 takes over its blklogwrites harness.

Before task 3: **#66** (`kai-bundle 1`) merges, so the new `fs` program needs no boot-config change (ADR 0000 §4.2), and **#72** and **#74** merge, since they edit the syscall dispatch and take call numbers first. **#74** (`Shm`) is needed by P2 itself only if S04-2 fails.

**Founder answers P2 needs.** Each row says what this plan builds until the founder answers:

| Question | Default this plan builds |
|---|---|
| ADR 0002 question 9: moves and overwrites you plainly ask for run at once, with undo? | They ask, as roadmap 3b says. A yes changes two rows' tier in task 11 and nothing else. |
| ADR 0004 question 3: keep undo history 30 days, dropping the oldest when the disk is under 10 % free, without asking? | Yes, as ADR 0004 §4 says. Emptying the trash is the one file action that asks. |
| ADR 0004 question 6: v0.1's filesystem is new code, proven by crash tests in CI and VMs, with real disks only from P7 | Acceptable for a VM release. |
| ADR 0002 question 8: how long the log keeps your requests on disk, and a `forget` command | An entry lives as long as its undo snapshot (30 days); `forget` waits for the answer. |
| ADR 0001 question 4: the AI may list and read any file in your home folder without asking | Yes, as roadmap 3b says. Nothing it reads can leave the machine without asking. |
| `csum_kind`: seahash-64 (#84's recommendation, #88) or SHA-256 | seahash-64, unless S04-2 finds a reason against it. |
| ADR 0009 question 4: may kaifs copy RedoxFS's allocator source with its MIT notice? | Until it is answered, kaifs keeps the buddy allocator S04-1 wrote for itself. |

## 3. The work, in order

P2 adds three lanes and borrows turns from the P1 lanes that are still running. Lane names follow [ADR 0000](../adr/0000-execution-plan.md) section 4; the storage lane S is split in two, so two sessions can work on it without touching the same files.

| Lane | Owns in P2 | State |
|---|---|---|
| **S-engine** | `kaifs/` (new), `xtask/src/fs/` (new) | New. Host-only until task 8. |
| **S-io** | `kernel/src/block.rs`, `kernel/src/virtio/blk.rs`, `kernel/src/nvme/`, `nvme/`, `kernel/src/blk_tests.rs`, `diskmap/` (new), `user/src/bin/fs/` (new), `kernel/src/process/syscall/fs.rs` (new area file), `xtask/src/cases/storage.rs` (new lane table) | New. Takes ADR 0000's lane D block files, which no running session holds, and lane S's server. |
| **P** packages | `kaipkg/` (new), `user/src/bin/pkg/` (new) | New in P2. P1's `pkg_*` rows stay A-core's. |
| **K** (P1, running) | `kernel/src/process/`, `kabi/src/handle.rs`, `kernel/src/handle/`, `captable/`, `supervise/` | Unchanged. Lends turns (section 3.4). |
| **A-core** (P1, running) | `agentabi/`, `shellcore/` | Unchanged. Takes tasks 11 and 12 after its P1 queue. |
| **Model lane** (P1, running) | `llmproto/`, `infer/src/`, `user/src/bin/llm/`, `termcore/`, `kernel/src/terminal.rs` | Unchanged. Lends one turn (task 10); the pack KV file runs beside. |
| **C** (P1, running) | `wit/`, `appabi/`, `wasmhost/`, `livecore/`, `compsdk/`, `components/` | Unchanged. Beside items only. |

### 3.1 Core tasks

Each row is one PR. "Needs" names what must merge first. Sizes are est.; line counts are ADR 0004's.

| # | Task | Lane | ADR | Needs | Size | Exit check |
|---|---|---|---|---|---|---|
| 1 | **Block write.** `block::Device` gains `write`, `flush` and `info` (volatile cache, flush support, atomic write unit); one lock per disk replaces the one over all disks; virtio-blk `write_blocks` and `flush`, with the device's features read before `VirtIOBlk::new`; NVMe WRITE (01h) and FLUSH (00h), VWC and AWUPF; a write-back cache without FLUSH is refused read-write; the "used read-only" line goes. S04-3's raw case moves in from its spike. | S-io | 0004 §6, 0007 | section 2 | M (~400) | `blk-write` on virtio-blk and NVMe; S04-3 raw: 0 violations over every flush point |
| 2 | **`diskmap` and the data disk.** A `no_std` GPT reader and writer grown from `xtask/src/image/gpt.rs`, with hostile tests; the dev and CI `kai-data` disk (section 4.1) for QEMU, and a third NVMe port with `kai-data.vdi` in VirtualBox, created once and never deleted with the OS disk; the kernel finds `kai-data` by type GUID, never by serial; a boot case may boot twice on one data disk (section 4.2) | S-io | 0004 §5, 0008 | 1 | M (~500) | Host and hostile tests; `blk-write`'s second boot reads back what the first wrote |
| 3 | **`Partition` and a stub `fs`.** `Kind::Partition` and its ceiling bit; PART_INFO, PART_READ, PART_WRITE and PART_FLUSH, numbered at merge and bounds-checked by the kernel; startup `Partition` handles only for the service `fs` running the program `fs`; an `fs` plan line with a stub server, started before `shell` | S-io, K's turn | 0004 §6-7, 0001 D2 | 2; #66, #72, #74 | M (~300) | Case `part`: the ESP and slots are unreachable, a block out of range is refused, no other process holds a `Partition`; S04-2's figures re-run on the real calls |
| 4 | **kaifs core**, written fresh from S04-1's design: format 1's superblock and both rings, `BlockPtr`, the mount rules and `EARLIEST_SUPPORTED`, the B+tree with 4 KiB nodes and a `fmt` byte, the buddy allocator, commit (records, FLUSH, both rings, FLUSH), reuse decided at replace time with the N+2 margin, group commit, `BlockDev`, `RecordingDisk` and the crash enumerator | S-engine | 0004 §1-3 | #88; `csum_kind` | L (~3,500 + ~2,000 tests) | Host: at least 1,000,000 crash states, 0 violations; S04-1's deliberate bugs are all caught; 4 KiB overwrites at most 3.0x |
| 5 | **Namespace.** INODE, DIRENT (a 64-bit name hash, collisions chained in the item), DATA records of up to 128 KiB, XATTR; `mkdir`, `rename`, whole-file atomic write; `check` and `check --repair`'s rules; golden image 1; hostile-input tests | S-engine | 0004 §2 | 4 | L (~2,000) | Host: crash states over namespace operations, 0 violations; the golden image mounts and checks |
| 6 | **Volumes and undo.** `system`, `home` and `models`; SNAPSHOT and UNDO records; `begin(action_id)`, `commit` and `abort`; `restore(txn)` with `name (restored)`; `changes(since)` by birth generation; `history(path)`; the trash with `info.cbor`; retention (30 days, or oldest first under 10 % free); GC in slices of at most 5 ms | S-engine | 0004 §4-5 | 5 | M (~1,200) | Host: a restore never overwrites a later change; `changes` reads only changed subtrees; one commit spans volumes |
| 7 | **`cargo xtask fs`**: `mkfs`, `import`, `ls`, `cat` and `check`; the blklogwrites log reader and replayer | S-engine | 0004 §8, S04-3 | 4; grows with 5 and 6 | M (~900) | Host tests; runs on Windows without WSL |
| 8 | **The `fs` server.** `kai-fs 1` as versioned CBOR (minicbor 2.3.0) over 8 KiB IPC, at most 7 KiB of data a message, or over `Shm` and KRNG v1 if S04-2 failed; the `Dir` and `File` kinds, server-backed, with badge `(volume, object id)` and rights READ, WRITE, ENUMERATE, CREATE and DELETE; path-beneath lookup ported from cap-std; the 64 MiB CLOCK block cache; a mutating call answered only after its commit is flushed; a restarted `fs` serves the same badges | S-io, K's turn | 0004 §6-7, 0001 | 3, 6 | L (~1,200) | `fs-reboot`, `fs-restart` |
| 9 | **The power cut.** S04-3 with file actions: 200 actions behind blklogwrites; the host replays every flush and FUA point and checks each; commit latency p50 and p99 (two flushes each) for ADR 0002's typing-to-done table. `fs-powercut`: xtask kills QEMU 20 times mid-write and boots again each time (section 4.2) | S-io | 0004 S04-3 | 7, 8 | M | `fs-powercut`, 0 violations |
| 10 | **Model store.** `/system/models/<sha256>.gguf`, SEALED in 2 MiB records; `xtask fs import --model` checks SHA-256 once; MODEL_FILL, numbered at merge: the kernel checks bounds, length, the GGUF header and FNV-1a64, fills the reserved runs and zeroes the tail; until then MODEL_MAP answers Busy and `llm` retries; `kai-modelhint 1` in block 0 of `kai-data` | S-io, model lane's turn | 0004 §8, 0008 | 8 | M (~500) | `model-store`; every case on the `kai-model` disk passes unchanged |
| 11 | **File rows.** Read: `fs_list`, `fs_read`, `fs_history`, `fs_changes`. Run, with undo: `fs_create`, `fs_trash`, `fs_restore`. Ask, with undo: `fs_move`, and `fs_write` over an existing file (run, if ADR 0002 question 9 is approved). Ask, no inverse: `fs_empty_trash`. `Inverse::Fs { txn }` restores through `Host`; a script is one kaifs transaction; the `dir` and `selection` State fields | A-core | 0002 P2 row, 0004 §6, 0010 | 8; P1 task 15 | M | `fs-act`, `fs-undo` |
| 12 | **Settings, rules and the log on disk.** `actlog 1` appended to `/system/log` in the same commit as its action, with `pack_id`; standing rules and grants in `/system/grants` as `kai-grants 1`; `/system/settings/<area>.cbor`; `undo` across a reboot | A-core, K's turn for grants | 0002 D7, 0001 D8, 0004 §5 | 11 | M (~600) | `fs-undo-reboot` |
| 13 | **Package store and generations.** `/system/pkg/store`, `gen/<n>` in `kai-gen 1`, `current` and `good`, failed hashes and GC; the switch, its grants and its log line in one transaction; S11-2 run over kaifs's transaction API | P | 0011 §5, S11-2 | 8, 12; P1 task 16 | M | `pkg-store`; S11-2's harness and `pkg-spike` |
| 14 | **P2 close.** Every exit case in `ci.yml`'s shards; decisions rows; the roadmap's P2 status; ADR 0004's text brought in line with S04-2 and S04-3 | – | – | all | S | Section 1's exit criteria |

**Critical path:** 4, 5, 6, 8, then 9 and 11 side by side, then 14: six steps. The engine (tasks 4-6, about 6,700 lines) is the long pole. Tasks 1-3 run beside it and must merge before 8; tasks 7, 10, 12 and 13 run beside the rest.

### 3.2 Beside lanes during P2

These start in P2 because their ADRs put work here, but P2's exit does not wait for them:

- **Lane D, drivers** (ADR 0007): KRNG v1 in `kabi::ring` with host-tested indices; the device ABI (DEV_CLAIM, DEV_MAP, DEV_DMA, DEV_IRQ, DEV_CFG, NOTIFY_WAIT, RING_ATTACH, RING_NOTIFY); `usbd` in ring 3 if S07-1 passes; NVMe queues and MSI-X completions; the i6300esb watchdog for ADR 0008's fault path. KRNG v1 moves onto P2's critical path only if S04-2 fails.
- **Lane U, updates** (ADR 0008): the five-partition image (ESP, `kai-r`, `kai-a`, `kai-b`, `kai-data`) written through `diskmap`; the `Update` kind with the SLOT_* and BOOTREC_* calls; `KaiBootState` and boot probation, with "`kai-data` mounted" as a required check; `/system/live.state`, `/system/trust` and `/system/updates`; the `update` snapshot before a new slot's first boot; `updated` with `kaituf` and the update disk.
- **Lane T, terminal compiler** (ADR 0012): `promptcore`; `:save` and `:load` in `/home/user/c/`; `#include` read by the shell under the request's grant and passed in a `Shm` when over 8 KiB; source journals on disk.
- **Model lane** (ADR 0010 §7): the pack KV file through `infer::segments`, at most two files in `/system/models/cache/`. It moves to the front of P2 if S10-3's cold prefill is over 10 s.
- **Lane C, components** (ADRs 0003, 0004 §5): the `kai:app` `storage` interface, backed by `fs` with a folder per app; component checkpoints in `/system/state/<component>/`.
- **Lane K**: the `powerbox` service (ADR 0001's P2 row) and the `Device` kind for `usbd`.

### 3.3 Running sessions in parallel

| Session | Lanes | Tasks | Starts |
|---|---|---|---|
| 1 | S-engine | 4, 5, 6, 7, strictly in order | At approval. Host-only, so it holds no file another lane needs. |
| 2 | S-io | 1, 2, 3, then 8, 9 and 10 | At approval. Task 8 waits for task 6. |
| 3 | A-core | 11, 12 | When its P1 queue (tasks 15-17) is done |
| 4 | P | 13; S11-2's host half early, over design C's three-primitive model | When P1 task 16 has merged |

With P1's four sessions still running, P2 starts with sessions 1 and 2 only. That makes six at once, above ADR 0000's 4-5; the founder sets the number. Sessions 3 and 4 start as P1's lanes free up.

Tasks with two halves pass between sessions at a merged PR, never mid-branch: 3 and 8 (lane K's kinds, then S-io), 10 (S-io, then the model lane's retry), 12 (A-core, then lane K for grants).

### 3.4 Where P2 takes a hot-file turn

| File | P2's change | Task | Turn |
|---|---|---|---|
| `kabi/src/handle.rs`, `kabi/src/cap.rs` (lane K) | `Kind::Partition`, then `Kind::Dir` and `Kind::File`, each with a ceiling bit claimed at merge | 3, 8 | After #72 and #74 merge. Lane K writes or reviews the kind part of each PR. |
| `kernel/src/process/syscall/mod.rs` (lane K) | One table line for the new area file `syscall/fs.rs`, which S-io owns | 3 | Same turn |
| `kernel/src/process/` spawn and startup handles (lane K) | `Partition` handles only for service `fs` running program `fs` | 3 | Same turn |
| `kernel/src/handle/` (lane K) | `Dir` and `File` handles backed by `fs`'s endpoint | 8 | One PR, reviewed by lane K |
| `kabi/src/call.rs` | PART_INFO, PART_READ, PART_WRITE, PART_FLUSH; later MODEL_FILL | 3, 10 | Named in the PR, numbered at merge after whatever #72 and #74 take |
| `kernel/src/main.rs` init and the boot config | Finding the data disk; the `fs` program | 1, 3 | After #66, so `fs` needs no Limine change |
| `kernel/src/model.rs`, `kabi/src/model.rs`, `user/src/bin/llm/` (model lane) | MODEL_FILL; MODEL_MAP's Busy; `llm`'s retry | 10 | One PR with the model lane's agreement, between its decode-speed PRs |
| `xtask/src/qemu.rs`, `xtask/src/disk.rs`, `xtask/src/vbox/` | The data disk, blklogwrites, boots that kill QEMU | 2, 9 | One PR at a time; every lane's boots run through these files |
| `xtask/src/cases/mod.rs` | `TestCase` gains the data-disk and second-boot fields; one line joins `storage::CASES` | 2 | Once, in task 2 |
| `agentabi/`, `shellcore/` (A-core) | File rows, `Inverse::Fs`, the on-disk log | 11, 12 | A-core's own tasks, after its P1 queue |
| `.github/workflows/ci.yml` | None. New cases join the shards, and the blklogwrites replay runs inside xtask's case. | – | – |
| `Cargo.toml`, `Cargo.lock`, `docs/decisions.md` | New crates `kaifs`, `diskmap`, `kaipkg` (and `seahash` 4.1.0, MIT, if `csum_kind` stays seahash-64); decision rows | each | ADR 0000 §4.2: re-run cargo, append only |

## 4. Decided in this plan

No ADR settles these. Each is small and can change in review.

1. **The data disk in dev and CI.** Until lane U's five-partition image and P8's installer exist, `kai-data` sits on a disk of its own: a GPT with one `kai-data` partition starting at 2 MiB, made by `cargo xtask fs mkfs`. `cargo xtask run` keeps one at `target/kai-data.img` across runs (`--data-disk PATH` to choose it, `--fresh-data` to start over). Boot cases get a fresh one each. The `kai-model` disk stays for every case that does not test the model store.
2. **Cases that reboot.** A case may name a second boot on the same data disk, with its own command line and expected lines; `blk-write`, `fs-reboot`, `fs-undo-reboot` and `pkg-store` use it. `fs-powercut` has xtask kill QEMU at a seeded random moment during a write loop, 20 times, booting again each time. Killing QEMU is not a host power cut, because the host keeps what QEMU had written, so the blklogwrites replay stays the proof and the kills are a smoke test, as ADR 0004 says of VirtualBox.
3. **The AI's reach in files.** `fs_*` rows take paths under `/home/user`; a relative path starts there, and lookup is path-beneath, so `..` cannot leave it. `/system` is reached only through its own rows (settings, rules, packages, models), never through `fs_*`.
4. **Create never overwrites.** `fs_create` on a path that exists is refused with one line that names `fs_write`, so the run tier can never replace a file.
5. **Folders made at first mount.** `xtask fs mkfs`, and `fs` when they are missing, make `/home/user`, `/home/.trash`, `/system/settings`, `/system/state` and `/system/pkg`.

## 5. Spikes

None runs a real model, and none runs on the founder's PC except S04-3's VirtualBox half, which the founder starts.

| Spike | ADR | Where | Length (est.) | Gates | When |
|---|---|---|---|---|---|
| **S04-2** `fs` over IPC | 0004 | GitHub Actions, QEMU under KVM, no model | 2-3 days | Tasks 3 and 8: copied IPC or `Shm` with KRNG v1; `csum_kind` before format 1 freezes | Now, on `spike/fs-ipc` |
| **S04-3** flush honoured | 0004 | GitHub Actions under KVM and TCG with blklogwrites; VirtualBox on the founder's PC, storage only | Raw half 3-4 days; the file half is task 9 | Task 1 (raw writes), task 9 (file actions); any user data on disk | Raw half now, on `spike/blk-flush` |
| **S11-2** store-switch | 0011 | GitHub Actions: host harness, then `pkg-spike` under KVM and TCG | About 1 week | Task 13; whether ADR 0004 needs a multi-step transaction before P2 closes | Host half once task 6 lands, or earlier over design C's model; the boot case with task 13 |
| **S07-1** ring-3 driver | 0007 | GitHub Actions under KVM and TCG | About 2 weeks | Lane D's `usbd` (beside) | Beside |
| **S10-3** cold prefill | 0010 | One KVM job with the real model in the guest, from S02-2's runs | Days | Whether the pack KV file moves to the front of P2 | From the model lane's S02-2 runs |

S04-2's pass mark is at least 50 MB/s each way; below it, bulk data moves to `Shm` with KRNG v1 rings, and KRNG v1 joins the critical path ahead of task 8. S04-3's is 0 violations at every flush and FUA point; on VirtualBox, 20 of 20 hard power-offs mount clean, or VirtualBox is documented as not power-safe.

## 6. Tests

- **Host tests** for every new crate (`kaifs`, `diskmap`, `kaipkg`, `xtask fs`), with hostile tests through `testkit/hostile.rs` for every new parser (section 1).
- **The crash harness lands with the engine** (task 4), not after it, and every later kaifs PR keeps it at 0 violations over at least 1,000,000 states. It grows with each task: namespace operations in task 5, volumes, restore and GC in task 6.
- **Golden images.** Format 1's images are committed with task 5; every later build must mount them and pass `check`.
- **CI boot cases** under KVM and TCG with the mock model: section 1's exit set and `part`. They join `ci.yml`'s shards with no workflow edit.
- **The replay.** S04-3's blklogwrites log is replayed on the host inside xtask's case, so it needs no new job. QEMU's blklogwrites is GPL-2.0-or-later and runs only in CI (ADR 0004 question 5, decided 2026-10-22).
- **Undo per row.** Every run-tier file row has an undo test, and every inverse is compare-and-set (ADR 0002 D7).
- Each run records its runner and run id in its PR.

## 7. Estimate

- **Core:** 14 PRs. About 11,000 non-test lines in all (ADR 0004's estimate, est.), most of it in tasks 4-6 and 8.
- **Beside lanes in P2:** lanes D, U, T, C, K and the model lane, about 12-18 PRs plus S07-1 (est.).
- **Timing:** ADR 0004 estimates 10-14 weeks for one stream. With S-engine and S-io in parallel, P2's length is set by the engine (tasks 4-6) and then tasks 8, 9 and 11: about 8-11 weeks (est.).

**Start with these two, in parallel, once the plan is approved:**

1. **Task 4, kaifs core** (session 1). It is the long pole and needs nobody's files.
2. **Task 1, block write** (session 2), carrying S04-3's raw case. Then tasks 2 and 3 as lane K's turn comes.

## 8. Risks

| Risk | What we do |
|---|---|
| A crash bug in new filesystem code | The harness lands with the engine and grows with it; golden images; S04-3's replay of real flush order in CI. FSCQ, a verified filesystem, still shipped a crash bug in its unverified glue (CrashMonkey, OSDI '18), so the server and the rows are replayed too, not only the engine. |
| S04-2 under 50 MB/s | Bulk data moves to `Shm` with KRNG v1 rings. KRNG v1 (lane D) joins the critical path before task 8; weeks in P2 (ADR 0000 §7.1). |
| 4 KiB overwrites at 2.96x, close to the 3.0x bar | S04-1's finding 3: a leaf with a 128-key fan-out adds headroom. Task 4 measures it again. |
| Hot-file waits on lane K (#72, #74) | Tasks 1, 2 and 4-7 touch no lane K file. Only tasks 3 and 8 wait for a turn. |
| VirtualBox ignores NVMe flushes | Set a flush key if one exists, else document VirtualBox as not power-safe (ADR 0004's fallback). QEMU's replay stays the proof. |
| `fs` costs a process slot and 64 MiB | 16 slots today; ADR 0003 raises the limit to 64 in the first PR that needs it. `fs`'s pages raise `OTHER_SERVICES_PAGES`. |
| Six sessions at once | The founder sets the number; P2 starts with two sessions and adds two as P1's lanes close. |
| GitHub Actions time | P2 needs no real-model job. Boot cases only, and spikes trim their CI to the jobs they measure. |

## 9. Deferred, and where it goes

- **P3:** fonts, themes and keymaps as files (ADR 0005); `Shm` shared with surfaces if S04-2 failed.
- **P4:** the files, notes, settings and model manager apps; editor drafts that survive a crash; per-app storage in the apps; `c>` sessions (ADR 0012).
- **P5:** verified downloads with resume into `/system/models` and the package store; `kaituf`'s cached metadata in `/system/trust` read online.
- **P6:** the `linux` volume with `root.img` and `home.img`; smaller records for VM disk files; the `Shm` bulk path from `vmm` to `fs`.
- **P7:** NVMe writes on real disks; 4Kn namespaces; hostile images from USB sticks; ext4-view import; FAT or exFAT exchange.
- **P8:** the installer writes the layout and formats `kai-data`; encryption at rest (ADR 0004 §9); accounts; image fuzzing; a security review.
- **Waiting on the founder:** section 2's questions.
