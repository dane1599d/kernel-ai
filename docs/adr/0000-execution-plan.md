# ADR 0000: Execution plan for P1-P8

Accepted 2026-10-22 (proposed 2026-10-21; updated 2026-10-22 with the founder's answers, section 2.1). The founder approved P0 on 2026-10-22.

This is the order of work once the founder approves ADRs 0001-0012. It decides nothing that an ADR decides; it sequences them. Repo lines are cited on main at ec5ecff. Nothing was built, run or measured for this plan. Sizes marked "est." are estimates. The P0 decision sheet (2026-10-21) wins where it differs from a single ADR.

The founder, 2026-10-21: "we have a whole entire road map to complete". So every phase should be moving, each in its own lane, without rushing a decision and without work that will be thrown away. The rule that makes this possible: a lane may build a host-tested crate as soon as its ADR is approved, but it wires that crate into the kernel or a service only when the phase it depends on has landed.

## 1. Before anything: finish and merge what is open

These need no new approval and come first, because every lane rebases on them.

| Item | Action | Why first |
|---|---|---|
| #16 DHCP, DNS, ping (merged 2026-10-21) | Follow-up PR with ADR 0001's change: call 37 gated on a new `cap::RESOLVE` bit, not `cap::NET` (a yes to that one item is enough) | Otherwise `llm` can carry data out in DNS names (0001, "Changes") |
| #18 virtio-gpu 2D, #19 USB input over xHCI (both merged) | No change; no ADR asks for one | S05-1 needs #18; S07-1 needs #19 |
| #21 a quieter screen (open) | Merge as it is; its log-only channel and `:log` are where P1's action log shows first (0002) | Fewer conflicts in the shell's audit code for P1 |
| Follow-up to #15 (seamless actions) | One PR of about 150 lines (est.): 0001's `fixed`/`floor` rows and audit words, 0002's compare-and-set undo and interim consent check, 0003's non-`'static` names in `of_script`, the new Ask wording | P1's registry inherits these; doing them now keeps P1 smaller |
| Follow-up to #17 (Ember terminal) | `Kind::of` returns `Log` for rows starting with `[`; ring-3 pieces cut to fit `PIECE_CHARS`; busy sign painted at once (0001, 0002) | The ask tier rests on ring 3 never drawing a prompt |
| Follow-up to #14 (decode speed) | Jump-forward in `infer/src/chat.rs` with a warm-against-fresh test (0002) | Largest latency lever for calls, and it helps today's envelope |
| Proof-of-concept items (roadmap.md:133, :149) | M10a's recording and proof (kernel-ai#2), faster prefill (kernel-ai#4), the fluency gate | v0.1 includes M10a (roadmap.md:153). M10b moves to P3-P4 (section 4) |

## 2. What must be approved first, and the spikes that come first

### 2.1 Approval in five batches (0 to 4)

An ADR can be approved before its spike runs. Each ADR states its pass and fail rules in advance, so approving the ADR also approves its fallback. The batches follow what each phase needs.

| Batch | ADRs and questions | Unblocks | When |
|---|---|---|---|
| 0, licence answers | [0009](0009-licence.md) questions 2 (free class additions), 3 (smartstring, Noto) and 4 (copied files, C in the image, host-only tools); [0005](0005-graphics.md) question 2 (OFL fonts) | Four spikes that need C, GPL host tools or OFL fonts (2.3), and Rhai in P1 | Now |
| 1, the agent core | [0001](0001-capabilities.md), [0002](0002-agent-actions.md), [0010](0010-skill-pack.md), [0003](0003-app-model.md); 0002 questions 1, 2 and 9; 0010 question 1 (3,072 positions); 0011 question 1 (named installs) | All of P1 | Before P1 starts |
| 2, the system underneath | [0004](0004-storage.md), [0007](0007-drivers.md), [0008](0008-updates.md), [0011](0011-packages.md), the rest of [0009](0009-licence.md) (D1-D8, before any outside PR) | P2, and the driver and update lanes inside P1 | During P1 |
| 3, the surface | [0005](0005-graphics.md) after S05-1's screendumps (question 1, the look); [0012](0012-terminal-compiler.md) after S12-1 | P3, P4's `c>` | Before P3 |
| 4, existing software | [0006](0006-existing-software.md) after spike P0-06 (questions 1, 3, 4) | P6's full build | Before P6's hypervisor merges |

**Answered 2026-10-22.** Batch 0's licence questions are answered, except whether copied permissive source files count as free (0009 question 4), so the four spikes in 2.3 can start. In batch 1, the founder answered 0002 questions 1, 4, 5 and 6 (an Allow / Deny menu with "always allow" rules; "Taken as" for vague requests; English-only consent; pasted text is the user's), 0010 question 1 (3,072 positions), 0011 question 1 (named installs run) and 0003 question 4 (a Wasm-first hybrid). The OS licence (0009 question 1) is deferred to before v0.1, with no outside contributions until then. Batch 1 still needs: approval of ADRs 0001, 0002, 0003 and 0010 as a whole, and 0002 questions 2 and 9 (named sites; moves and overwrites).

Founder questions that do not block a phase stay open until their phase: for example 0004 question 4 (encryption, P8) and 0009 question 7 (a company, before v1.0).

### 2.2 Spikes that can run now

Spikes are throwaway branches named `spike/*`, never merged. They add no product code to main, so they need no approval. None runs a real model on the founder's PC. Real-model runs use a GitHub runner, as `fluency-model.yml` does.

| Spike | ADR | Where it runs | Rough length | Decision it settles |
|---|---|---|---|---|
| S04-1 kaifs crash core | 0004 | Host tests, GitHub Actions and locally | 10 working days | Our own filesystem, or the RedoxFS fork (5 more days) |
| S01-2 tier corpus | 0001, 0002 | Host tests, GitHub Actions | about 1 week (est.) | Whether the request is enough consent |
| S01-1 handle-run | 0001 | Boot cases, GitHub Actions KVM and TCG; TCG locally in WSL2 | about 2 weeks (est.) | `scriptrun` per request, pooled, or in `shell` |
| S10-0 token count | 0010 | GitHub Actions, tokeniser only | 2-3 days (est.) | Real pack sizes against the budget |
| S02-1 action gate with S10-1 | 0002, 0010 | GitHub runner, real Qwen3-4B on the host engine; about 24 runner-hours (0010's est.) | about 2 weeks to build (est.) | Tagged call or script only; `dsl-1` or another format |
| S02-2 with S10-3, in-guest latency | 0002, 0010 | One KVM job on a GitHub runner, real model in the guest | 2-3 days (est.) | The 2-4 s typing-to-done target; whether the KV file moves up |
| S03-1 same bytes, two engines | 0003 | Host tests and a comphost boot case, GitHub Actions KVM | 2 weeks | wasmi plus `appabi`, or Wasmtime in v0.1 |
| S03-2 Wasmtime in ring 3 | 0003 | Same runners | 1 week | Whether the second backend is ready when a trigger fires |
| S08-1 efi-ab | 0008 | GitHub Actions (300 KVM, 100 TCG cycles); founder's VirtualBox VM with no model disk | about 2 weeks (est.) | Limine one-shot, or our own EFI stage |
| S08-3 with S11-1 kaituf-0 | 0008, 0011 | GitHub Actions, host tests and `tuf-bench` | about 1 week | One TUF client; JSON or line format |
| S09-1 release dry run with S08-2 | 0009, 0008 | GitHub Actions, ubuntu-24.04 and one windows-2025 job | about 1 week (est.); under 30 min a run | Notices, reproducible slot images under 256 MiB |
| S11-2 store-switch (host half) | 0011 | Host tests over C's three-primitive model, GitHub Actions | about 1 week | One transaction per package switch |
| S06-1 nesting census | 0006 | 20 GitHub Actions runs; once in VirtualBox | 2 days | Whether CI has SVM with NPT |
| S07-1 ring-3 driver | 0007 | GitHub Actions KVM and TCG | about 2 weeks (est.), after #19 merges | `usbd` in ring 3, or xHCI stays in the kernel |
| S12-3 Grain gate | 0012 | Host tests and a KVM boot case | days (est.) | Rhai 1.27.0's Grain or AST by default |

### 2.3 Spikes that waited for one licence answer (batch 0, answered 2026-10-22)

| Spike | ADR | Waits on | Where | Rough length |
|---|---|---|---|---|
| S05-1 compose and look | 0005 | OFL fonts (0005 question 2); #18 merged | GitHub Actions KVM; TCG locally; screendumps for the founder | 2-3 weeks (est.) |
| S07-3 uACPI glue | 0007 | C in the kernel and clang (0007 question 1, 0009 question 4) | GitHub Actions, both accelerators | 1-2 weeks (est.) |
| S12-1 cc-wasm | 0012 | xcc (C) and gcc or node in CI (0012 question 5, 0009 question 4) | GitHub Actions KVM and TCG | about 2 weeks (est.) |
| S06-2 and S06-3 | 0006 | mmdebstrap and `mke2fs` in CI, a GPL guest run in CI only (0006 question 2) | GitHub Actions KVM; TCG locally for function | about 4 weeks with S06-1 |

Later spikes, run at their phase: S02-3 (P1 boot cases), S04-2 and S04-3 (P2's first PRs), S05-2 (P3 entry), S07-2 and S07-4 (before P7), S10-2 (before v0.2), S11-3 (P5 entry), S12-2 (P4, before the model writes C).

## 3. The order in which P1-P8 start

```
 batch 0-1 approvals + P1 spikes
            |
            v
 P1 agent core ---- handles, registry, one door, Shm (0001, 0002, 0010)
   |    |    \
   |    |     +--> P6a hypervisor groundwork (svm, Vm kind) ...........+
   |    |                                                                |
   |    +--> P8a gates that start early: DCO, cargo-deny, fuzz targets   |
   v                                                                     |
 P2 files ---- block write, kaifs, fs, KRNG v1, usbd, bootslot, pkg store |
   |    \                                                                |
   |     +--> P5 networking (needs P2 for downloads) ----+               |
   v                                                     |               |
 P3 desktop ---- Display, surfabi, desktop, uikit        |               |
   |    \                                                |               |
   |     +--> P7a real-hardware groundwork (S0, S07-2, S07-4, amdvi)     |
   v                                                     v               v
 P4 apps, packages, c>  ==> v0.1 (VM)          P6b Linux micro-VM, browser ==> v0.2
                                               P7b founder's PC ==> v0.3
                                               P8 production ==> v1.0
```

| Phase | Starts when | Must have before its exit | Rough size (roadmap.md:139-147) |
|---|---|---|---|
| P1 Agent core | Batch 1 approved; section 1 merged | S01-1, S01-2, S02-1, S10-0, S10-1, S03-1 results | weeks (10-15 commits) |
| P2 Files and storage | P1's handle steps 1-3 merged; S04-1 passed; batch 2 approved | S04-2, S04-3, S11-2 | weeks to months |
| P3 Desktop | P1's `Shm` merged; P2's `fs` serving files; batch 3 approved | S05-1 passed; S07-3 for power-off | months |
| P4 Built-in apps | P3's `uikit` drawing one window | S12-1 for `c>`; S12-2 for model-written C | months, overlapping P3 |
| P5 Networking | P2's `fs` and store; P1's `NetDest` kind | S11-3 | weeks to months |
| P6 Existing apps | Groundwork after P1's `Shm` and P2's KRNG; full build after batch 4 | P2 large files, P3 foreign surface, P5 network | many months to a year or more |
| P7 Real hardware | Groundwork during P3-P4; board work after v0.1 | S0 (board IDs), S07-2, S07-4 | months for one PC |
| P8 Production | Gates in P1; the rest after v0.2 | everything | ongoing |

**Critical path to v0.1:** batch 1 approval, then P1 handles (0001 steps 1-3), `agentabi` and `agentcore::execute`, `Shm`; then P2 block write, kaifs and `fs`; then P3 `Display`, `surfabi`, `desktop` and `uikit`; then P4's apps as signed packages; then the release (S09-1, a signed TUF target). Nothing else may sit on this path: work that is not on it runs in another lane.

**Long poles:** P6 (route C is 7-15 person-months, fallback A about 20; 0006 question 4) and P7 (drivers for one board). Both start groundwork early, so v0.2 and v0.3 do not wait for v0.1 to finish before they begin.

## 4. Parallel lanes

Each lane owns its directories. Two sessions never edit the same file at once. A lane that needs a change in another lane's file asks that lane's session, or waits for a hot-file turn (4.2). A sensible number of sessions at once is 3 in P1 and 4-5 from P2; the founder sets it.

### 4.1 Lanes and what they own

A name below that is not on main at ec5ecff is a new crate or program an ADR names, or arrives with #18 (`gfxcore/`, `kernel/src/display/`) or #19 (`xhci/`, `kernel/src/usb.rs`).

| Lane | Phases | Owns (crates and directories) |
|---|---|---|
| K Kernel objects | P1, then any new kind | `captable/` (new), `kabi/src/handle.rs` (new), `kernel/src/handle/` (new), `kernel/src/process/supervision.rs`, `supervise/` |
| A Agent | P1, then rows for every phase | `agentabi/` (new), `shellcore/`, `llmproto/` (with `llmproto/src/pack/`), `infer/src/chat.rs`, `user/src/bin/shell*`, `user/src/bin/llm/`, `xtask/src/fluency/` |
| C Components | P1, P4 | `appabi/` (new), `wit/` (new), `wasmhost/`, `livecore/`, `compsdk/`, `components/`, `user/src/bin/comphost.rs`, `editcore/` |
| D Drivers | P1-P3, P7 | `pci/`, `devmatch/`, `devkit/` (new), `kernel/src/block.rs`, `kernel/src/virtio/`, `kernel/src/nvme/`, `nvme/`, `kernel/src/apic.rs`, `kernel/src/idt.rs`, `acpitables/`, `xhci/` (#19), `kernel/src/usb.rs`, `user/devlib/`, `user/src/bin/usbd` (new), later `amdvi`, `netd`, `audiod` |
| S Storage | P2, P4 | `kaifs/`, `diskmap/` (new), `user/src/bin/fs` (new), `xtask/src/fs*` (new) |
| U Updates and trust | P1-P2, P4, P8 | `bootslot/`, `kaituf/` (new), `kernel/src/efi.rs` (new), `user/src/bin/updated` (new), `xtask/src/image/`, `xtask/src/esp.rs`, `.github/workflows/release.yml` (new) |
| P Packages | P1-P2, P4-P6 | `kaipkg/` (new; the reader moves from `livecore/src/artifact.rs:156-224` in one agreed PR with lane C), `user/src/bin/pkg` (new) |
| G Graphics | P3-P4 | `surfabi/`, `gfx2d/`, `look/`, `deskcore/` (new), `gfxcore/` (#18), `kernel/src/display/` (#18), `user/src/bin/desktop` (new), `termcore/` |
| T Terminal compiler | P1-P4 | `wasihost/`, `kailink/`, `promptcore/` (new), `third_party/xcc/`, `user/src/bin/cc` (new) |
| V Virtual machines | P1 (bits), P6 | `svm/` (new), `kabi/src/vm.rs` (new), `kernel/src/vm/` (new), `cpufeat/`, `user/src/bin/vmm` (new), `guest/` (new: `kai-guest` and the image recipe) |
| N Network | P5 | `kernel/src/net/` (#16), new TLS and HTTP crates, `user/src/bin/netd` later (with lane D) |
| L Licence and release | P1, P4, P8 | `deny.toml`, `about.toml`, `xtask/src/notices.rs`, `xtask/notices.json`, `LICENSES/`, `.github/workflows/dco.yml` (new), `docs/third-party-notices.md` |
| M Memory | P3-P4 | M10b: `mm/`, `kernel/src/memory/` |
| R Research | any | `spikes/*` on `spike/*` branches only |

### 4.2 Shared hot files and how lanes take turns

| File | Today | Rule |
|---|---|---|
| `kabi/src/lib.rs` | 386 lines; cap bits, `GRANTABLE`, ABI constants | P1's first lane K PR splits it into modules (`handle`, `ring`, `vm`, `surface`), so a lane adds its own module and one `pub mod` line. Kind-ceiling bits and error codes are claimed at merge, in merge order (sheet section 2) |
| `kabi/src/call.rs` | 125 lines; numbers 0-37 | A new call is named in its PR and numbered at merge: the PR that merges first takes 38, and so on. That PR also updates the decisions row on call numbers (decisions.md:123). Never two open PRs holding the same number |
| `kernel/src/process/` | `syscall.rs` is one `match` of 37 arms (`:30`) | Lane K owns the directory in P1. Its first PR moves dispatch arms into per-area files (`syscall/{core,handle,fs,display,dev,vm}.rs`, proposed here, no behaviour change). After that a lane edits only its own area file and adds one line to the table |
| `xtask/src/cases.rs` | one `TEST_CASES` array (`:112`) | Same first week: split into per-lane tables joined in one place (proposed here). Until then, append at the end and rebase |
| `.github/workflows/ci.yml` | 152 lines; boot cases are dealt by `--shard` (`:44-79`) | New boot cases need no `ci.yml` edit. New jobs go in their own workflow file. A `ci.yml` change is its own PR, one at a time |
| `docs/decisions.md` | rows by milestone | Append only, one row per decision, in the PR that makes it. Never reorder. The founder's rows are edited only on the founder's word |
| `Cargo.toml` members, `Cargo.lock` | one workspace | A lane adds its crates in its first PR; a lock conflict is resolved by re-running cargo, never by hand. Every PR lists new crates and their licences (founder rule, 2026-10-21) |
| Limine config and `kernel/src/main.rs` init | one module per program | Lane U lands `kai-bundle 1` early in P1 (0008), so a new program (`fs`, `desktop`, `usbd`, `vmm`, `cc`, `pkg`, `updated`) no longer changes the boot config |

## 5. The first PRs of each phase

Each row is one PR. "Case" means a boot case run under KVM and TCG in GitHub Actions; "host" means `cargo test` there and locally.

### P1 Agent core

| # | PR | ADR | Touches | Exit check | Unblocks |
|---|---|---|---|---|---|
| 1 | Make room: split `kabi`, the syscall dispatch and `cases.rs`; `kai-bundle 1` | 0001, 0008 | lanes K and U files above | Every existing case passes unchanged | Parallel lanes |
| 2 | Handles: `kabi::handle`, `Rights`, BadHandle and Revoked, `ABI_INFO`, the "KAI" note; `captable`; per-slot tables; startup handles from `caps`; `need()` on handles (0001 steps 1-3) | 0001 | `captable/`, `kabi/`, `kernel/src/handle/`, `kernel/src/process/` | `captable` host tests; all cases pass on handles; S01-1's M1 cost recorded | Every new kind; P2 |
| 3 | `agentabi` with the 7 M8 rows, `tune`, service and component rows; `API`, `TIERS`, Rhai bindings and `:help` generated from it; `llmproto::pack` with `dsl-1` and its golden | 0002, 0010 | `agentabi/`, `shellcore/`, `llmproto/` | Host: consistency check, pack golden, OS rows at most 1,024 tokens; the 50-task script gate unchanged | Rows for every phase |
| 4 | `agentcore::execute`, consent, the tagged decoder with jump-forward, the in-memory `actlog 1` and `undo` | 0002 | `shellcore/`, `infer/`, `user/src/bin/shell*` | Cases `act-plain`, `act-run`, `act-ask`, `act-undo`, `act-rollback`, `act-inject`, `act-never` with mock answers (roadmap.md:140's exit) | P1 exit; every later row |
| 5 | Handle IPC, grants and request nodes, `ONCE`, `scriptrun`, `Shm` with user unmap and TLB shootdown (0001 steps 4-6) | 0001 | lane K files, `kernel/src/memory/user.rs` with lane M's agreement | S01-1's M2-M6 thresholds as cases | P3 surfaces, P6 guest memory, P2 bulk path |

Beside these, in other lanes: `appabi` and `wit/` after S03-1 (lane C); `pci`, `devmatch`, MSI-X and `devkit` (lane D, about 4,500-6,000 lines over P1-P3 by 0007's est.); `bootslot` and `efi.rs` (lane U); `pkg_*`, update, model and `system_about` rows (lane A); Rhai 1.27.0 (lane A), then `wasihost` and `kailink` (lane T); `cpufeat` SVM bits (lane V); the `dco`, `deny` and `licences check` gates (lane L).

### P2 Files and storage

| # | PR | ADR | Touches | Exit check | Unblocks |
|---|---|---|---|---|---|
| 1 | Block `write`, `flush`, `info`, one lock per disk, for virtio-blk and NVMe; S04-3's raw block-write case | 0007, 0004 | `kernel/src/block.rs`, `kernel/src/virtio/`, `kernel/src/nvme/`, `nvme/` | S04-3 under blklogwrites: 0 violations over every flush point | Any user data on disk |
| 2 | `diskmap` and the five-partition GPT (ESP, `kai-r`, `kai-a`, `kai-b`, `kai-data`) | 0004, 0008 | `diskmap/`, `xtask/src/image/` | Host tests; the `image` case finds `kai-data` by type GUID | `fs`, A/B slots |
| 3 | `kaifs` format 1, written fresh from S04-1's design; golden images; `cargo xtask fs check` | 0004 | `kaifs/`, `xtask/src/fs*` | Host: at least 1,000,000 crash states, 0 violations | `fs` |
| 4 | `fs` service, `Partition`, `Dir` and `File` kinds, `PART_*` and `MODEL_FILL` calls, `kai-fs 1` | 0004, 0001 | `user/src/bin/fs`, lane K's area files | Case `fs-reboot` (data survives a reboot); S04-2 at least 50 MB/s each way | File rows, store, KV file |
| 5 | `fs_*` rows with `Snapshot` and `Trash` inverses; `actlog 1` and `kai-grants 1` on disk | 0002, 0004, 0001 | `agentabi/`, `shellcore/` | Cases `fs-undo` (undo restores a deleted file) and a power cut mid-write (roadmap.md:141's exit) | P2 exit |

Beside these: KRNG v1, the device ABI and `usbd` after S07-1 (lane D); the i6300esb watchdog; `KaiBootState`, `updated` and `kaituf` with the update disk (lane U); the package store and generations (lane P, S11-2's boot case); `promptcore` and `#include` (lane T); the pack KV file (lane A).

### P3 Desktop

| # | PR | ADR | Touches | Exit check | Unblocks |
|---|---|---|---|---|---|
| 1 | `Display` and `Input` kinds; `DISPLAY_CLAIM`, `DISPLAY_FLUSH`, cursor calls, `INPUT_READ`; revoke on Ctrl+Alt+F12 | 0005, 0007 | `kernel/src/display/`, lane K's area file | Case `display-claim`: claim, revoke, console repaints | `desktop` |
| 2 | `surfabi` (`kai-surface 1`) with hostile tests | 0005 | `surfabi/` | Host and fuzz tests | Apps, `vmm`'s foreign surface |
| 3 | `gfx2d` and `look`: tokens, contrast test, coverage tables | 0005 | `gfx2d/`, `look/`, `termcore/` (theme move) | Host tests; screendump goldens within S05-1's budgets | `uikit` |
| 4 | `deskcore` and `desktop`: windows, focus, panel, the bottom `kai>` line | 0005 | `deskcore/`, `user/src/bin/desktop` | Case `desk-windows`: windows open, move and close | P3 exit |
| 5 | `uikit` in comphost, `kai:ui@0.1.0`, the agent view, `ui_*` and `window_close` rows | 0005, 0003, 0002 | lanes G and C, by agreement; `agentabi/` | Case `desk-agent`: the agent opens notes and acts inside it (roadmap.md:142); S05-2's 300-token view | P4 |

Beside these: uACPI power-off and the power button after S07-3 (lane D); M10b before native memory-pressure work (lane M); the foreign `Surface` for P6 (lane V).

### P4 Built-in apps (v0.1)

| # | PR | ADR | Touches | Exit check | Unblocks |
|---|---|---|---|---|---|
| 1 | `kaipkg`; built-in apps as signed `.kpkg` in the image's repository; the boot manifest stops naming notes (`livecore/src/boot.rs:13`) | 0011, 0003 | `kaipkg/`, `livecore/`, `xtask/` | Case `pkg-install`: install from the image, `undo` restores the previous generation | Every app |
| 2 | Terminal app on `uikit` with typed spans | 0005, 0003 | `components/terminal`, `termcore/` | Agent-driven per-app action test | `c>` |
| 3 | Files, notes and settings apps; settings shows "what each app can do" and revokes | 0001, 0004, 0005 | `components/*` | Agent-driven per-app tests (roadmap.md:143) | v0.1 |
| 4 | System monitor and model manager; the editor (M9d steps 10-12) | 0007, 0008, 0003 | `components/*`, `editcore/` | Per-app tests | v0.1 |
| 5 | The `c>` prompt: `cc`, sessions, `:dis`, `undo`, `cc build` installing from `local` | 0012, 0011 | lane T files | Case `c-prompt`; S12-1's budgets; model-written C only if S12-2 passes | v0.1 |

Then the release: S09-1 passed; 0009's D1-D8 in place; v0.1 signed as a TUF target through `release.yml` and `kernel-ai-updates` (0008).

### P5 Secure networking (v0.2)

| # | PR | ADR | Touches | Exit check | Unblocks |
|---|---|---|---|---|---|
| 1 | `NetDest` kind; RESOLVE becomes a right on it; `:resolve` and `:ping` become read rows | 0001, 0002 | lane K, `kernel/src/net/`, `agentabi/` | Case: an app without a `NetDest` is refused | Per-app network |
| 2 | TLS 1.3 (rustls 0.23.46 with a RustCrypto provider) and an HTTP client | 0009 | lane N crates | Host: HTTPS mock with a test CA refuses a bad certificate | Downloads |
| 3 | Chunked, resumable, verified downloads (`kai-chunks 1`) into the store and `/system/models` | 0008, 0011, 0004 | lanes N, P, U | Case: a cut connection resumes; a bad hash is refused (roadmap.md:144) | Online installs, model setup |
| 4 | The online repository over TLS; the network grant replaces `pkgrepo=`; S11-3's GHCR mirror | 0011 | lane P | Case: online `pkg_install` and `undo` | v0.2's `install` |
| 5 | Open-world rows, the host table, `Hold`; S10-2 and the index level | 0002, 0010 | lane A | S02-1's bait probes stay at 0 false runs | P6 rows |

### P6 Running existing apps (v0.2)

| # | PR | ADR | Touches | Exit check | Unblocks |
|---|---|---|---|---|---|
| 1 | `cpufeat` SVM, NPT and NRIPS bits; S06-1's CI census kept (can land in P1) | 0006 | `cpufeat/`, a census workflow | Host tests; the census job | `svm` |
| 2 | `svm` crate: VMCB layout, exit codes, NPT builders | 0006 | `svm/` | Host tests | The run loop |
| 3 | `Vm` kind, `kabi::vm` 1, `kernel/src/vm/` run loop, the `VMEXIT` KRNG ring | 0006, 0001, 0007 | lane V; lane K's area file | Case `vm-hello`: a tiny guest writes to serial | `vmm` |
| 4 | `vmm`: virtio block, net and console; boots Debian's kernel from the `linux` volume | 0006, 0004 | `user/src/bin/vmm` | Case: a static Linux binary runs (roadmap.md:145's first step) | GUI apps |
| 5 | virtio-gpu in `vmm` as a foreign `Surface`; `kai-guest`; `browser_open`, `media_play` and the other rows | 0006, 0005, 0002 | lanes V, G, A | A GUI app shows, then a self-hosted 720p30 VP9 page plays (never youtube.com in CI) | v0.2 |

### P7 Real hardware (v0.3)

| # | PR | ADR | Touches | Exit check | Unblocks |
|---|---|---|---|---|---|
| 1 | S0: the board model and PCI IDs (0007 question 3); `devmatch` entries | 0007 | `devmatch/` | Host test over the founder's ID list | Driver choice |
| 2 | GOP colour masks and EDID scale (`kernel/src/console/mod.rs:117-119`) | 0005, 0007 | `kernel/src/console/`, `kernel/src/display/` | Host test; a look on the PC with no model disk | Display on the PC |
| 3 | `amdvi` and `iommu=require`, after S07-2 | 0007 | lane D | S07-2's case under QEMU `amd-iommu` | Ring-3 drivers on real hardware |
| 4 | `netd` for RTL8125 or I226-V, after S07-4 and the firmware answer | 0007 | lane D | `devkit` host tests over a fake device; a check on the PC | Network on the PC |
| 5 | USB stick boot on the founder's PC: NVMe writes with VWC and AWUPF, `usbd` hubs, x2APIC beyond 8 CPUs | 0007, 0004 | lane D | The v0.3 checklist, first with no model disk | v0.3 |

Running the model on the PC's bare metal is the founder's call, asked at P7 (roadmap.md:146's exit).

### P8 Production (v1.0)

| # | PR | ADR | Touches | Exit check | Unblocks |
|---|---|---|---|---|---|
| 1 | Continuous fuzzing for every parser the ADRs list (`kai-grants 1`, `kai-app 1`, `actlog 1`, `surfabi`, kaifs images, `kaituf`, `kaipkg`); targets land with each parser from P1 | all | `testkit/`, `*/tests/hostile.rs`, a fuzz workflow | Each target runs a fixed time per week with no crash | Security review |
| 2 | Installer: GPT with unique GUIDs, formats `kai-data` | 0008, 0004 | lanes U, S | Case: install to a blank disk and boot it | v1.0 |
| 3 | Secure Boot per slot; a key-rotation drill | 0008 | lane U | S08-1's harness with signed slots | v1.0 |
| 4 | Accounts, per-account logs, encryption at rest | 0004, 0002 | lanes S, A | Cases per account | v1.0 |
| 5 | SBOM (cargo-cyclonedx 0.5.9), the VMX backend, third-party publishers under `packages` | 0009, 0006, 0011 | lanes L, V, P | Release checklist (roadmap.md:147) | v1.0 |

## 6. Packages, the terminal compiler and the releases

**Package manager (ADR 0011).** P1: the `pkg_*` rows, the generation inverse, the consent rule and the package-line parser over the image's repository and RAM generations. P2: `/system/pkg` on kaifs, generations and the one-transaction switch. P4: built-in apps as signed packages, installs from the image and a local mirror, `cc build` into `local`. P5: the online repository (v0.2). P6: the `linux-layer` package and `apt install` routed into the guest. P8: other publishers. `kaituf` is shared with updates (lane U owns it).

**Terminal compiler (ADR 0012).** P1: Rhai 1.27.0 and the Grain gate, `more` replies, the NXE check, S12-1, `wasihost` and `kailink` as host-tested crates. P2: `promptcore`, `#include`, `:save` and `:load`. P3: multi-line input. P4: the `c>` prompt, sessions, `:dis`, `undo` and `cc build`, in v0.1. Model-written C waits for S12-2.

| Release | Contents after these ADRs |
|---|---|
| v0.1, in a VM (P1-P4, M10a) | Handles and grants; the agent core with tagged calls, tiers in code, a log and `undo`; kaifs with snapshots, trash and file undo; A/B slots with rollback and offline updates from an update disk; `usbd` in ring 3 if S07-1 passes; power-off if uACPI is approved; the desktop with the AI panel; terminal, files, notes, settings, system monitor, model manager and editor as signed packages; `install` from the image and a local mirror; the `c>` prompt; MPL-2.0 with the DCO if 0009 question 1 keeps it, and `NOTICES.txt` |
| v0.2 (P5-P6) | TLS, verified resumable downloads, the online repository and `install`, in-OS model setup with a GHCR mirror; the Linux micro-VM with Firefox ESR as one foreign surface; the browser and media rows; `audiod` on virtio-snd; the index level of the skill pack |
| v0.3 (P7) | The founder's PC from a USB stick: GOP, NVMe writes, USB hubs, `netd`, `amdvi` with `iommu=require`, HD Audio, SVM on the Ryzen 7 7700X so the browser runs there |
| v1.0 (P8) | Installer, Secure Boot, accounts, encryption, fuzzing and security reviews, SBOM, the VMX backend, third-party packages, `kai:app` frozen if 0003 question 3 says so, broader hardware |

## 7. Risks to the plan, and what happens if a spike fails

### 7.1 Spike fallbacks

Each fallback was set in its ADR before the spike runs. None changes the phase order; the "cost" column says what moves.

| Spike fails | Fallback (ADR) | Cost to the plan |
|---|---|---|
| S01-1 M3 over 30 ms | Pooled `scriptrun`; if that fails, scripts stay in `shell` under request nodes (0001) | Days |
| S01-2 adversarial case | Fix the rule, not the threshold; over 15 prompts per 100 benign cases, add pickers (0001) | Blocks P1 exit until fixed |
| S02-1 under 90% exact | v0.1 keeps `script` as the only form; tiers, consent, labels and the log stand (0002) | None to the order |
| S02-2 over 4 s | Profile before P1's call path merges (0002) | Weeks inside P1 |
| S03-1 differences or over 16.7 ms | Wasmtime backend into v0.1 if S03-2 passes; if wit-component fails, re-judge A against C (0003) | Lane C slips; others continue |
| S04-1 | RedoxFS fork, B's 5-day spike; everything above the engine stays (0004) | 1-2 weeks in P2 |
| S04-2 under 50 MB/s | `Shm` with KRNG v1 rings for bulk data in P2 (0004) | Weeks in P2 |
| S04-3 on VirtualBox | Set a flush key if one exists, else document VirtualBox as not power-safe (0004) | None |
| S05-1 scenes | vello_cpu; then a GPU or virgl path; compositing in the kernel re-judged; a frame deadline in `classes` (0005) | P3 slips; the protocol and agent view do not change |
| S06-1 to S06-3 | Fallback A, `lxabi` (about 20 PM against 7-15); mostly Intel runners puts VMX into P6 (+1-2 PM) (0006) | v0.2 slips; P1-P4 unaffected |
| S07-1 | xHCI stays in the kernel on MSI-X (0007) | None to v0.1 |
| S07-2 | Host tests plus a hand check on the PC (0007) | None |
| S07-3, or no to C | ACPICA (also C); with no C at all, reboot works but power-off does not on real PCs (0007) | P3's power-off item drops |
| S07-4 | The P7 NIC stays in the kernel behind `devkit` (0007) | None |
| S08-1 | Our own EFI stage with GPT attribute bits, chainloading Limine (0008) | Weeks in lane U |
| S08-2 over 512 MiB | Revisit slot sizes before any install exists (0008) | Days |
| S08-3 or S11-1 | Line-format signed metadata (design C), which also changes 0008 (0011) | 1-2 weeks in lane U |
| S10-1 | Rework rules and examples; the index level moves into v0.1 (0010) | Weeks in P1 |
| S10-3 cold prefill over 10 s | The pack KV file moves to the front of P2 (0010) | Reorders P2 only |
| S11-2 | 0004 adds a multi-step transaction before P2 closes (0011) | Weeks in P2 |
| S11-3 | Hugging Face stays the only source; Docker Hub is the mirror (0011) | None |
| S12-1 | A warm compiler snapshot, then a prelude cache, then our own C-subset front end (0012) | `c>` may slip past v0.1; nothing else does |
| S12-2 | The model does not write C; `c>` is for people (0012) | None |
| S12-3 | AST stays the default (0012) | None |

### 7.2 Risks to the plan itself

| Risk | What we do |
|---|---|
| GitHub Actions time. M10a's boot cases were once refused on billing (roadmap.md:71); S10-1 alone is about 24 runner-hours (est.) | Run the costly real-model spikes one at a time; keep KVM jobs gated; record each run's id and runner in its PR |
| One approver. Every batch waits on the founder | Batches are small and ordered; spikes and host-only crates run while a batch waits |
| Hot-file conflicts as lanes grow | The P1 make-room PR (section 5); claim-at-merge for numbers and bits; one CI change at a time |
| Licence answers stall work | Batch 0 first; each ADR names the crate or tool it waits on, and the free-class rule covers the rest |
| Memory. The 4B with a 3,072 context needs a 3,840 MiB VM; the browser beside it about 6.9 GiB (est.) | 0010 question 1 and 0006 question 8 settle it before P1 and P6 |
| The dev PC. No KVM locally yet; C: nearly full | TCG in WSL2 for function; timings only from CI under KVM; builds on D: or E: |
| Rushing a format. kaifs format 1, `kai-surface 1`, `kabi::ABI` and `kai:app` are hard to change | Each freezes only after its spike and golden tests; ABIs and formats carry versions from the first release (roadmap.md:131) |
| Scope creep inside a phase | A PR does one row of section 5. Anything else becomes a new row, decided here first |

When a spike's result changes a phase, this plan is revised in a PR before that phase's next code lands.
