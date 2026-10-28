# P4 plan: built-in apps, packages, `c>` and the v0.1 release

Status: approved by the founder on 2026-10-27 with its defaults (MPL-2.0 with the DCO, three YubiKeys when release work starts, keep "Kernel AI" with a trademark search before v0.1). Amended the same day: the in-OS assistant is named Kai (section 4, decision 9). It plans the work that the accepted ADRs (`docs/adr/`, accepted 2026-10-22) and the execution plan ([ADR 0000](../adr/0000-execution-plan.md) §3, §5 and §6, P4 rows) put in P4: the P4 rows of [ADR 0011](../adr/0011-packages.md) (packages), [ADR 0012](../adr/0012-terminal-compiler.md) (the `c>` prompt), [ADR 0003](../adr/0003-app-model.md) (every built-in app on `kai:app`), [ADR 0005](../adr/0005-graphics.md) (apps on `uikit`, the other themes), [ADR 0010](../adr/0010-skill-pack.md) (app groups in the pack) and [ADR 0008](../adr/0008-updates.md) and [ADR 0009](../adr/0009-licence.md) (the signed release). P4 is the last phase before v0.1, "an AI-native OS you can try", in a VM (roadmap §3b, Releases). Section 4 lists the few things this plan decides itself. Nothing here was built or run for this plan. Sizes marked "est." are estimates.

## 1. Goal and exit

**Goal** (roadmap P4 row, the Packages and Terminal compiler tracks, and the v0.1 line): the terminal, files, notes, settings, system monitor and model manager apps and the full-screen editor, each declaring its actions; all of them shipped as signed packages that install from the image and from a local mirror; the `c>` prompt beside the editor; and v0.1 released as a signed TUF target.

What the founder sees at the end of P4, in a VM (QEMU or VirtualBox) booted from the v0.1 release image:

- **A desktop with real apps.** Terminal, Files, Notes, Settings, System Monitor, Models and Editor open from `kai>` or from the app catalogue, and each is a window drawn through `uikit`.
- **The AI works inside every app.** "show me what's using the most memory" opens System Monitor sorted by memory. "move the groceries note to my documents folder" moves it through Files, with undo. Each app's actions are its own `kai-actions 1` rows, so the agent calls them by name.
- **Apps are packages.** `pkg list` shows the seven built-ins as generation 1, checked against a signed index at boot. `install weather` from the local mirror installs at once (or asks, if it needs a new permission), and `undo` puts the previous set back.
- **The catalogue.** The `kai>` panel's Apps sheet lists installed apps and the ones the image and the mirror offer, with summary, size, licence and the permissions each needs.
- **TempleOS's compiler at the prompt.** `:c` switches to `c>`, where C runs as you press Enter; `:dis`, `:defs`, `undo` and `cc build` work. The model writes C only if spike S12-2 passes.
- **A release anyone can try.** A GitHub Release with the image, `NOTICES.txt`, the source tarball, a signed `SHA256SUMS` and notes naming the model to download; the same bytes are a TUF target in `dane1599d/kernel-ai-updates`.

**Exit criteria.** All of these pass in CI, under KVM and TCG, with the mock model keyed by request. The roadmap's exit, "per-app action tests driven by the agent", comes first:

| Boot case | Shows |
|---|---|
| `app-terminal` | The agent opens the terminal app, runs a command in it through its actions, and reads the typed output spans back |
| `app-files` | The agent lists a folder, moves a file (ask), trashes one (run), and `undo` restores both |
| `app-notes` | The agent makes a note; it survives a reboot through `kai:app` storage |
| `app-settings` | The agent switches the theme (`appearance_set`, run, with undo); Settings shows what each app may do, and a revoked grant leaves the app refused until `undo` |
| `app-monitor` | The agent sorts by memory and names the top process; stopping a process asks |
| `app-models` | The agent lists models; a fixture model imported from an attached disk is checked by SHA-256 against the signed `models` role; `model_use` switches and `undo` switches back |
| `app-editor` | The agent opens a component's source at a line in the full-screen editor; an edit builds in the background; Ctrl-click on a source link opens its line |

This plan adds cases for the rest of the deliverable:

| Boot case | Shows |
|---|---|
| `pkg-image` | The built-ins boot as generation 1 from the image's signed repository, and the boot manifest no longer names notes; a tampered target in the bundle is refused by name and the rest still boots |
| `pkg-install` | Install from xtask's test-signed local mirror through 192 KiB `Range` GETs, `undo`, install again from the store with no fetch; ADR 0011's five tamper cases refused by name |
| `desk-catalog` | The Apps sheet lists installed and available apps; Enter on an available one installs it with the same consent as a typed `install`; an index summary written as an instruction is shown as text and changes no tier |
| `c-prompt` | `:c`, a definition, a call, a redefinition that switches the caller, `:dis`, `undo`, `:rhai`; a `kai` import's effect goes through `agentcore::execute` and is undone |
| `c-build` | `cc build` writes a component that installs from the `local` repository, signed by the device key, and runs; `undo` removes it |
| `release-dry` | (A workflow, not a boot case.) `release.yml` on a throwaway `v0.1.0-rc.0` tag with test keys: two clean builds give one SHA-256, the five assets are drafted, and a CI VM accepts the target from an update disk and refuses a tampered copy |

Host gates in CI: hostile tests through `testkit/hostile.rs` for every new parser (`kai-theme 1`, the catalogue's index fields, `promptcore`, the mirror client); S05-2's 300-token view for each app's real tree, not only its P3 mock; `cargo deny`, `cargo xtask licences check` and `cargo xtask notices --check` (already on main, #41, #42).

**Real-model gates, on GitHub only.** P4 changes model input: each app's action group joins the pack, `appearance_set` and the model rows join the OS rows, and, if S12-2 passes, the `c-1` card. So task 15's PR runs `fluency-model.yml` (at least 47/50) and `actbench-model.yml` with per-app tasks, and S10-0's count is re-run on the v0.1 registry. No P4 gate runs a model on the founder's PC.

## 2. Entry, and what comes before P4's first PR

**Entry conditions** ([ADR 0000](../adr/0000-execution-plan.md) §3, P4 row, and what the P4 tasks lean on):

| Condition | State on main at e825115, 2026-10-27 |
|---|---|
| P3's `uikit` drawing one window (P3 task 9) | Not yet. P4's app tasks wait for it; P4's host halves, spikes and release work do not (below) |
| P2's `fs` serving files (P2 task 8) and the package store on disk (P2 task 13) | Not yet. Task 13's `kaipkg` is host-tested in #143 (open) |
| S12-1 passed, for `c>` | Met: closed PR #73, verdict pass under KVM, release build (200-line prelude p50 41-44 ms; 1,000-line p50 191-211 ms; compiler peak 1,152 KiB; Kai C 13-15x faster than Rhai AST). `cc`'s plan line becomes `pages=3680` |
| S12-2 passed, for model-written C only | Not run. It runs in P4 (section 5) and gates task 14 alone |
| ADR 0012 approved (batch 3) | Met: accepted 2026-10-22 |
| P1's `pkg_*` rows and RAM generations | Met: #100 (`pkg-local` case) |
| Notes as `app:notes@1.0.0` with `kai.actions` rows | Met: #98 |
| `wasihost` and `kailink` | Met: #78, #80. `promptcore` (P2 beside, lane T) is not on main |
| `kaituf` (spike S08-3 with S11-1) | Not run; no `kaituf/` on main. Lane U's P2 beside item. Task 1 needs it |
| A/B slots, `updated` and the update disk (lane U, P2 beside) | Partly: `bootslot`'s next-boot rule is host-tested (#96); S08-1 is open (#128); `updated` is not on main. Task 17 needs them |
| ADR 0009's release terms | Partly: `deny.toml` and cargo-deny in CI (#41), `NOTICES.txt` and `cargo xtask notices` (#42), `LICENSES/`. Missing: the root `LICENSE` (waits on D1, question 1), `CONTRIBUTING.md`, `SECURITY.md`, `TRADEMARKS.md`, the `dco` job, the 0BSD examples file, one workspace version, SPDX headers, `PRODUCT_NAME` |

**Before P4's entry, once this plan is approved**, these can start, because they are host-only, spikes, or release work that touches no running lane's file (ADR 0000's rule: build host-tested crates early, wire them in when the phase underneath lands):

1. **S09-1 with S08-2**, the release dry run (section 5). Lane L with U.
2. **S12-2**, C fluency, on a GitHub runner (section 5). Lane T.
3. **Task 16a**, the release terms that do not wait on the licence (section 3.1).
4. **Task 1's host half**: xtask packs each built-in app as a `.kpkg` and writes a test-signed TUF repository; host tests only.
5. **Task 12's host half**: the `cc` service's logic over `wasihost`, `kailink` and `promptcore`, host-tested.
6. **Task 11's host half**: `editcore`'s full-screen model (buffer, viewport, pager, source links), host-tested.

**Founder answers P4 needs.** Each row says what this plan builds until the founder answers. The licence comes first because v0.1 cannot be tagged without it.

| Question | Default this plan builds |
|---|---|
| **1. The OS licence** (ADR 0009 D1 and question 1; deferred by the founder to before v0.1). Section 6 summarises the three options | **MPL-2.0 with the DCO and no CLA** (ADR 0009's option C, its proposal). Task 16b writes it; nothing else in P4 waits for it until the tag |
| 2. Keys (ADR 0008 question 1, ADR 0011 question 4): buy three YubiKeys (firmware 5.7 or later), one kept off-site; each package publish needs your touch | Yes. Until they arrive, every build is signed by a throwaway test root and nothing is published to `stable` |
| 3. Online keys for snapshot and timestamp (ADR 0008 question 2): a cloud KMS account, or Ed25519 file keys in a protected GitHub environment | File keys in a protected `release` environment for v0.1 (no cost, no account); a KMS before v1.0. The ADR leaves this open; this is the cheaper start |
| 4. The name and counsel (ADR 0009 question 6): D8 puts a clearance search before the v0.1 tag | Keep "Kernel AI" for the project and the OS; the assistant is Kai (decision 9). The search covers both names, and the tag waits for it. If no counsel is funded, tag `v0.1.0-rc.N` only, which publishes nothing to `stable` |
| 5. Model weights in the release (ADR 0009 question 9) | No weights in the image or the release. The notes name the pinned GGUF and its SHA-256; Models imports it from an attached disk and checks it (task 10) |
| 6. v0.1 install scope (ADR 0011 question 8): the image and a local mirror only, internet installs in v0.2 | Yes |
| 7. Model-written C (ADR 0012 question 2): once S12-2 passes, run tier, or always shown first | Run tier, as for Rhai: every `kai` import still goes through `agentcore::execute` with its own tier, and the program is in the log and `:why`. You said "if i tell it to do something it should just do it" |
| 8. Builds in the OS without a git commit (ADR 0012 question 4), for `cc build` | Yes, under the source-hash provenance line, installed through `local` |
| 9. Copied permissive source files (ADR 0009 question 4; ADR 0009's P4 row says in-OS C builds wait on it) | They count as free, each with its SPDX header and an `Imported-from:` trailer |
| 10. Restarts for updates (ADR 0008 question 3) | Only when you say "restart"; Settings offers a time window, off by default |

ADR 0008 question 5 (about 2.3 GiB reserved for the slots) and ADR 0003 question 3 (`kai:app` stays 0.x, frozen per release, as P3 assumes) keep their ADR defaults; v0.1 freezes `kai:app@0.1` and `kai:ui@0.1.0` for the release.

## 3. The work, in order

P4 adds three lanes and continues three that already exist. Lane names follow [ADR 0000](../adr/0000-execution-plan.md) §4.1 and the P2 and P3 plans.

| Lane | Owns in P4 | State |
|---|---|---|
| **AP-1** apps | `apps/terminal/`, `apps/files/`, `apps/monitor/` (new), `xtask/src/cases/apps.rs` (new lane table) | New |
| **AP-2** apps | `apps/settings/`, `apps/models/` (new) | New. Adds its cases to AP-1's table by rebase, one line each |
| **E** editor | `editcore/`, `user/src/bin/edit.rs`, `apps/editor/` (new) | New to a session. `editcore` was lane C's in ADR 0000 and is in no running lane's list |
| **P** packages (from P2) | `kaipkg/`, `user/src/bin/pkg/`, `xtask/src/pkgrepo/` (new: the packer, the test-signed repository and the mirror) | Continues after P2 task 13 |
| **T** terminal compiler (from P1) | `wasihost/`, `kailink/`, `promptcore/`, `third_party/xcc/`, `user/src/bin/cc/`, `xtask/src/cc/` | Continues |
| **U+L** updates, licence and release | Lane U: `bootslot/`, `kaituf/`, `kernel/src/efi.rs`, `user/src/bin/updated`, `xtask/src/image/`, `xtask/src/esp.rs`, `.github/workflows/release.yml` (new). Lane L: `deny.toml`, `about.toml`, `xtask/src/licences.rs`, `xtask/src/notices.rs`, `xtask/notices.json`, `LICENSES/`, `NOTICES.txt`, the root `LICENSE`, `CONTRIBUTING.md`, `MAINTAINERS`, `SECURITY.md`, `TRADEMARKS.md`, `.github/workflows/dco.yml` (new) | One session for both in P4: the release is where they meet |

None of these is a running lane's file:

- **K** keeps `kernel/src/process/`, `kabi/src/handle.rs`, `kernel/src/handle/`, `captable/`, `supervise/`.
- **A-core** keeps `agentabi/` and `shellcore/`.
- **The model lane** keeps `llmproto/`, `infer/`, `user/src/bin/llm/`, `look/`, `gfx2d/` and `termcore/`.
- **S-io** keeps `block`, `nvme`, `diskmap` and `user/src/bin/fs/`; **S-engine** keeps `kaifs/`.
- **P3's lane C** keeps `uikit`, `wit/`, `compsdk/` and `components/` (so notes stays in `components/notes`).
- **G-core** keeps `kernel/src/display/`, `kernel/src/input/`, `surfabi/`, `deskcore/` and `user/src/bin/desktop`.

P4 takes turns from each of them (section 3.4).

### 3.1 Core tasks

Each row is one PR. "Needs" names what must merge first. Sizes are lines with tests (est.).

| # | Task | Lane | ADR | Needs | Size | Exit check |
|---|---|---|---|---|---|---|
| 1 | **The signed image repository.** xtask packs each built-in app as `kai-pkg 1` and writes a TUF repository signed by a per-build test root (section 4, item 3) into `kai-bundle 1`; the `pkg` service starts with `kaipkg` and `kaituf`, verifies the repository and makes it generation 1; the supervisor starts apps from the current generation; the boot manifest stops naming notes (`livecore/src/boot.rs:13`, lane C's turn) | P, with U's and C's turns | 0011 §1-§5, §8, §12; 0008 §7 | P2 tasks 8 and 13; `kaituf` | M (~1,500) | `pkg-image` |
| 2 | **Installs from a local mirror.** The fixed `pkgrepo=IP:PORT` destination beside `llm=` and `build=`; 192 KiB `Range` GETs into `/system/pkg/store/.partial/`; `cargo xtask pkgrepo serve` serves a test-signed mirror with the same layout as `kernel-ai-updates`; `upgrade`; S11-2's boot half (install at most 3 s, rollback at most 100 ms under KVM) | P, K's turn for the destination | 0011 §8, S11-2 | 1 | M (~900) | `pkg-install` |
| 3 | **The app catalogue.** An Apps sheet in the `kai>` panel: installed apps from the current generation, then those the image and the mirror offer, with summary, size, licence and the grants each needs; Enter opens or installs through `pkg_install` with the same consent rule; index text is drawn as data | G-core, P's turn | 0011 §10, 0005 §8 | 2; P3 task 7 | M (~700) | `desk-catalog` |
| 4 | **Terminal app.** Today's shell in an app window on `uikit` with typed spans (row colour by kind stays in the kernel console only); multi-line input with `..> `; actions `terminal_run`, `terminal_read`, `terminal_clear`; the console stays the recovery screen | AP-1, model lane's turn (`termcore` for wasm32) | 0005 §4, 0003 | entry | M (~1,500) | `app-terminal` |
| 5 | **Files app.** Folders, preview, move, rename, trash and restore, all through P2's `fs_*` rows so tiers and undo are the OS's; actions `files_open`, `files_reveal`, `files_select` | AP-1 | 0004, 0002, 0005 | entry; P2 task 11 | M (~1,500) | `app-files` |
| 6 | **Notes on storage.** Notes keeps its notes in its folder through `kai:app` `storage` (P3's lane C beside item) and draws on `uikit` (P3 task 9); its rows from #98 stay | C's turn | 0003, 0004 §5 | entry; lane C's `storage` | S (~400) | `app-notes` |
| 7 | **Settings app.** Appearance; "what each app can do" from `kai-grants 1` with revoke (ADR 0001); standing rules and profiles from P3 tasks 11 and 12; an About page reading `system_about`; the update restart window | AP-2, A-core's turn for the grant rows | 0001, 0005, 0008, 0009 | entry; 8 | M (~1,500) | `app-settings` |
| 8 | **Themes.** Light, high-contrast and legible themes as `kai-theme 1` files; `appearance_set` (run, with undo) | Model lane (owns `look`), A-core's turn for the row | 0005 §8 | P3 task 6 | M (~1,200) | Host: contrast test on every theme; golden images per theme |
| 9 | **System monitor.** Processes, CPU, memory, the model's state and tokens/s from SCHED_STATS and `health`; actions `monitor_sort`, `monitor_select`; stopping a process is the OS's ask-tier row | AP-1 | 0007, 0002 | entry | M (~800) | `app-monitor` |
| 10 | **Model manager.** Models in `/system/models` and in the signed `models` role; import from an attached disk with the SHA-256 checked against the role; the licence shown (ADR 0009); `model_use` with probation | AP-2, model lane's turn | 0008 §9, 0009, 0004 §8 | entry; P2 task 10 | M (~1,000) | `app-models` |
| 11 | **The editor.** M9d steps 10-12 (decisions.md:113): the full-screen editor on `uikit` over `editcore` and the `edit` service, with background builds, a pager, and Ctrl-click from a source link to its line; drafts that survive a crash (P2's deferred item); the kernel's ed-style editor stays for recovery | E | 0003, 0005 | entry | L (~2,500) | `app-editor`; the `edit` and `terminal` cases unchanged |
| 12 | **The `c>` prompt.** The `cc` service (plan line `cc caps=serve pages=3680 stack_pages=512 burst_ms=5000`); sessions, the five conveniences, `:dis`, `:defs`, `:reset`, `undo`; need and resume through `agentcore::execute`; the shell's `:c` and `:rhai` modes and the terminal's `c>` and `..>` prompts | T, A-core's and the model lane's turns | 0012 D1-D6, D9 | 4; `promptcore` | L (~2,500) | `c-prompt`; S12-1's bars re-measured in the OS under KVM |
| 13 | **`cc build` and `local`.** Components built in the OS on `kai:app/component`, and apps with an empty `actions`; `kai.notices` with xcc's MIT notice; the provenance line; an Ed25519 device key made at first use; install from the `local` repository | T, P's turn | 0012 D7, 0011 §11 | 2, 12 | M (~900) | `c-build` |
| 14 | **Model-written C** (only if S12-2 passes). `script` gains `lang`; the `c-1` card rendered by `pack::render`; a fresh session per request with a trial on stub imports; `:keep` | A-core and model lane, with T | 0012 D5, D8; 0010 | 12; S12-2 passed | S (~500) | S12-2's bars; the Rhai gate stays at least 47/50 |
| 15 | **App groups in the pack.** Each installed app's `kai-actions 1` rows join the pack when its generation is current and leave when it stops; S10-0's count re-run on the v0.1 registry (if over a budget, the index level ships in v0.1, ADR 0010); per-app actbench tasks | Model lane | 0010 P4 row, 0011 §10 | 4-11 | M | Fluency at least 47/50; actbench with the per-app tasks |
| 16 | **Release terms** (ADR 0009's follow-ups not yet on main). **16a**, now: one workspace version `0.1.0-dev`; `PRODUCT_NAME`; `llmproto/src/examples.rs` under 0BSD; `SECURITY.md`; the `dco` job. **16b**, after question 1: the root `LICENSE`, `CONTRIBUTING.md`, `MAINTAINERS`, `TRADEMARKS.md`, the README's licence line, SPDX headers and `REUSE.toml` | L, with A-core's and the model lane's turns for `PRODUCT_NAME` in the prompts | 0009 D1-D9 | 16b: question 1 | S each | `cargo xtask licences check`; the prompt bytes unchanged (no fluency re-run, 0009 D8) |
| 17 | **The release pipeline.** `release.yml` on a `v*` tag in a protected `release` environment, actions pinned by SHA: build the slot and R images twice and require one SHA-256; the five assets of 0009 D7; the source tarball with the offline Limine path; an artifact attestation; tuf-on-ci's signing event for your YubiKey; then snapshot and timestamp, published to `kernel-ai-updates` | U+L | 0008 §8, 0009 D7 | S09-1; `updated`; 1 | M (~1,000) | `release-dry` |
| 18 | **The root ceremony and `kernel-ai-updates`.** The repository, its Pages and Releases; a 2-of-3 root on your YubiKeys; the `targets`, `packages` and `models` roles; the image's trusted root switched from the test root for release builds only | U+L, with you | 0008 §7, 0011 §3-§4 | 17; question 2 | S | Your touch signs `v0.1.0-rc.1`; a CI VM accepts it from an update disk |
| 19 | **P4 close and v0.1.** Every exit case in `ci.yml`'s shards; decisions rows; the roadmap's P4 status; a "try it" section in the README (VirtualBox and QEMU, the model download and import); `v0.1.0-rc.N` until the checklist passes, then `v0.1.0`, which you publish by hand | – | 0009 D7 | all; questions 1 and 4 | S | Section 1's exit criteria; the v0.1 checklist (section 3.5) |

**Critical path:** P3 task 9 (`uikit` draws one window) and `kaituf`, then task 1, then the app tasks side by side (the editor, task 11, and `c>`, task 12, are the longest), then task 15, then task 19. Task 17 runs beside the apps and must merge before 19. Your answers to questions 1, 2 and 4 gate task 19 only, so they can come as late as the last weeks, but the YubiKeys take time to arrive.

### 3.2 Beside lanes during P4

These start or continue in P4, but v0.1 does not wait for them:

- **Lane D**: `usbd` in ring 3 if S07-1 passed, and uACPI power-off if S07-3 passed. v0.1 ships them if they are on main at the tag (ADR 0000 §6 says "if"), and without them if not.
- **Lane M**: M10b demand paging. Not in v0.1 (ADR 0000 §6 lists M10a only).
- **Lane N, P5 groundwork**: `NetDest` and the rustls crates as host-tested crates. Nothing online lands before v0.1.
- **Lane V, P6 groundwork**: `svm` and the `Vm` kind. Nothing of the Linux guest ships in v0.1.
- **Lane C**: the Wasmtime backend only if S03-1's trigger fires; not expected.

### 3.3 Running sessions in parallel

| Session | Lanes | Tasks | Starts |
|---|---|---|---|
| 1 | P | 1's host half, then 1, 2, and P's halves of 3 and 13 | At approval (host half); in the OS after P2 task 13 |
| 2 | U+L | S09-1 with S08-2, 16a, then 17, 18, 16b | At approval |
| 3 | T | S12-2, 12's host half, then 12, 13, 14 | At approval (spike and host half) |
| 4 | AP-1 | 4, 5, 9 | At entry |
| 5 | AP-2 | 7, 10 | At entry |
| 6 | E | 11's host half, then 11 | Host half at approval if a session is free; the app at entry |
| – | K, A-core, model lane, C, G-core | Turns (section 3.4) | As each task reaches them |

At approval, sessions 1-3 start, while P2 and P3 lanes are still running. Sessions 4-6 start at entry, as P3's lanes free up. You set the number; with your 95% weekly usage cap, this plan suggests three P4 sessions until P3 task 9 merges, and no more than six P4 sessions at once.

### 3.4 Where P4 takes a hot-file turn

| File | P4's change | Task | Turn |
|---|---|---|---|
| `livecore/src/boot.rs`, `livecore/` (lane C) | The boot manifest stops naming notes; apps start from the current generation | 1 | Lane C writes or reviews |
| xtask's component build (lane C) | Builds components from `apps/<name>/` as well as `components/` | 4 | Lane C, once, before the first app |
| `uikit` (lane C) | Only if an app needs a widget beyond ADR 0005 §4's v0.1 set; none is expected | 4-11 | Lane C |
| `components/notes` (lane C) | Storage-backed notes | 6 | Lane C's own task |
| `agentabi/`, `shellcore/` (A-core) | `appearance_set`; the grant list and revoke rows for Settings; the reserved package-group list exported (ADR 0011 §2; `kaipkg::header::RESERVED` until then); `:c` and `:rhai`, the session commands, need and resume; `script(lang)` | 7, 8, 12, 14 | A-core's turns |
| `llmproto/`, `termcore/`, `look/` (model lane) | App groups in the pack; `c-1`; `prompt_width` for `c` and `..`; `termcore` built for wasm32; the themes | 4, 8, 12, 14, 15 | The model lane's turns, with the real-model gates |
| `deskcore/`, `user/src/bin/desktop` (G-core) | The Apps sheet; the theme switch | 3, 8 | G-core |
| `kernel/src/process/` (lane K) | The `pkgrepo=` destination, handed only to `pkg`; the `cc` and `pkg` plan lines | 1, 2, 12 | Lane K reviews |
| `kabi/src/call.rs` | None expected. If the mirror exchange needs a call, it is named in its PR and numbered at merge | 2 | Claim at merge |
| `kernel/src/main.rs` init and the boot bundle | The `pkg` and `cc` programs; the image repository as bundle files | 1, 12 | After `kai-bundle 1` (#66), so no Limine change |
| `xtask/src/qemu.rs` | The mirror's `pkgrepo=`; an attached model fixture for `app-models`; the update disk for `release-dry` | 2, 10, 17 | One PR at a time |
| `xtask/src/cases/mod.rs` | One line each for `apps::CASES` and the `pkg` and `c` tables | 1, 4, 12 | Once each |
| `.github/workflows/ci.yml` | None. New cases join the shards. `release.yml` and `dco.yml` are their own files | – | – |
| `Cargo.toml`, `Cargo.lock`, `docs/decisions.md` | The apps, `kaituf`, `promptcore`, the `cc` and `pkg` programs; decision rows | each | ADR 0000 §4.2: re-run cargo, append only |

### 3.5 The v0.1 checklist

v0.1 is tagged when all of these hold, and not before:

- Section 1's exit criteria pass on the tag's commit, under KVM and TCG.
- P1, P2 and P3 are closed, and M10a's exit test passes (it does: PR #7).
- A/B slots with rollback and offline updates from an update disk work (lane U, from P2).
- The built-ins are signed packages; `install` reaches the image and a local mirror.
- ADR 0009's D1-D8 are in place: the licence you chose, `NOTICES.txt` byte-identical in the image and the release, the source tarball rebuilding offline, the DCO job, both names cleared ("Kernel AI" and "Kai").
- The release is a TUF target signed by your root, in `kernel-ai-updates`.
- `release/seq` is bumped in the release PR, above the last release's: `release.yml`'s `seq` job refuses a tag otherwise (ADR 0008 §2, decided 2026-10-27).
- Fluency at least 47/50 and actbench pass on the tag's model input.

### 3.6 What v0.1 does not wait for

- **P5 networking** (v0.2): TLS, verified resumable downloads, the online repository and internet `install`, in-OS model setup (L7). In v0.1 the model is downloaded on the host and imported (question 5).
- **P6 existing apps** (v0.2): the Linux micro-VM, the browser and media rows, `apt install` into the guest. `apt install python3` keeps answering "Linux packages need the Linux layer (v0.2)".
- **P7 real hardware** (v0.3) and **P8 production** (v1.0): the installer, Secure Boot, accounts, encryption, third-party publishers.
- **Within P4:** model-written C if S12-2 fails (`c>` is then for people, ADR 0012); a package solver (ADR 0011 §6); the `tool` package kind; deltas; the index level of the pack, unless S10-0's recount needs it; `usbd` and power-off, if not ready; M10b.

## 4. Decided in this plan

No ADR settles these. Each is small and can change in review.

1. **P4's apps live in `apps/<name>/`.** ADR 0000 §5 says `components/*`, but P3's lane C owns `components/`, and two sessions never edit one directory. Notes stays in `components/notes`. xtask's component build learns the second root once (section 3.4).
2. **App rows are the app's own.** Each app declares `<app>_<function>` rows in its `kai-actions 1` (ADR 0003, ADR 0011 §2). Destructive work, such as moving a file or stopping a process, calls the OS's existing rows, so tiers and undo stay the OS's. The only new OS rows in P4 are `appearance_set`, the grant rows for Settings, the `model_*` rows ADR 0008 names, and `script(lang)` if S12-2 passes.
3. **CI signs with a throwaway root.** xtask makes a fresh test root per build for every `dev` image and boot case, so `pkg-image`, `pkg-install` and `release-dry` need no YubiKey. Only `release.yml` in the protected environment embeds your root. ADR 0009 D7 already says `dev` builds are never signed by the real root.
4. **The model is not in the release.** The notes name the pinned GGUF (unsloth/Qwen3-4B-Instruct-2507-GGUF, revision and SHA-256 from `xtask/models.json`); the user attaches it as a disk; Models imports it into `/system/models` and checks it against the signed `models` role (question 5).
5. **Index text is data.** The catalogue draws a package's summary (at most 80 bytes) and its action names as text in the panel's data style; they never reach the model as instructions and never change a tier (ADR 0011 §10, ADR 0010 question 3).
6. **`cc`'s compiler runs without wasmi's strict limits.** S12-1 found `cc.wasm` has 2,538 active data segments, over the strict limit of 1,000. Only the SHA-pinned `cc.wasm` runs that way; session code keeps strict limits.
7. **Per-app cases use the mock model.** Keyed by request, like P2's and P3's, so they run on every PR. The real model is measured by task 15's fluency and actbench runs.
8. **Tags.** `v0.1.0-rc.N` until section 3.5 holds, then `v0.1.0`. The workspace version is `0.1.0-dev` from task 16a (ADR 0009 D7); updates order by `release_seq`, never by SemVer.
9. **The assistant is Kai** (founder, 2026-10-27). The project and the OS stay "Kernel AI", the way a company names its model apart from itself. The name shows where the assistant speaks or is spoken of: ask cards, the prompt, the docs, and the model's self-introduction. Giving the model the name in its system prompt is a model-input change, so it is its own small PR in the model lane and passes the fluency gate (fluency at least 90%, as for any change to model input); the docs and UI strings follow in the PRs that touch them. The pre-v0.1 trademark search covers "Kai" as well as "Kernel AI".

## 5. Spikes

None runs a real model on the founder's PC, and none runs QEMU there.

| Spike | ADR | Where | Length (est.) | Gates | When |
|---|---|---|---|---|---|
| **S09-1** release dry run, with **S08-2** slot size and reproducibility | 0009, 0008 | GitHub Actions: ubuntu-24.04 runners and one windows-2025 job; no model | About 1 week | Task 17. Its fail rules are 0009's: any coverage gap, a Limine mismatch, a non-reproducible image or an offline rebuild that fetches is fixed before any tag | At approval |
| **S08-3** with **S11-1** `kaituf-0` | 0008, 0011 | GitHub Actions: host tests and `tuf-bench` under KVM and TCG | About 1 week | `kaituf`, so task 1. Fail: design C's line-format signed metadata, which also changes 0008 | Lane U's P2 beside item; at approval if it has not run |
| **S12-2** C fluency | 0012 | `fluency-model.yml` on a GitHub runner, real Qwen3-4B; never the founder's PC | Days to build, one runner job | Task 14 only. Bar: 70% first try, 90% after one repair. Fail: the model does not write C; `c>` is for people | At approval |
| **S10-0 recount** | 0010 | GitHub Actions, tokeniser only | 1-2 days | Task 15: whether the index level ships in v0.1 | When tasks 4-11's rows exist |

S09-1's bars: zero unknown licence entries; `NOTICES.txt` identical on Linux and Windows; one image SHA-256 on two fresh runners; an offline rebuild from the source tarball with the same SHA-256; `NOTICES.txt` at most 512 KiB; at most 90 s added to `lint-and-unit`; the whole run under 30 minutes.

## 6. The OS licence: the options in ADR 0009 D1

You deferred this to before v0.1 (decisions row "The OS licence", 2026-10-22). Whatever is chosen must be at least as generous as MPL-2.0, because the README said "MPL-2.0" publicly for about 23 hours (ADR 0009, Context). Until you decide, the repository stays "all rights reserved" and takes no outside contributions.

| Option | What it means | For | Against |
|---|---|---|---|
| **A. MPL-2.0 with a CLA** (Harmony 1.0, outbound Option Five) | Same file-level copyleft as C, but contributors sign a contract that lets you relicense later | Keeps every business option open, including dual licensing | Counsel-drafted CLA texts (weeks) before any outside PR; CLAs deter contributors (GitLab dropped its CLA in 2017, seL4 in 2020); a later relicence repeats HashiCorp's and Redis's trust problems |
| **B. Apache-2.0 OR MIT with the DCO** | Fully permissive; anyone may ship closed forks | Most adoption and crate reuse; cheapest to change later (moving to MPL is free, leaving it is not) | Gives up copyleft on the agent's tier table and capability checks for good; reverses `Cargo.toml`'s and the README's history |
| **C. MPL-2.0 with the DCO, no CLA** (the ADR's proposal) | Anyone who changes our files and ships them publishes those files; apps on top may use any licence; contributors add `git commit -s` | Matches `Cargo.toml:95`, `NOTICES.txt` and the public history; lowest contributor friction; `kabi`, `compsdk` and the WIT files become `MIT OR Apache-2.0`, so app authors meet only permissive terms | No relicensing without every contributor's consent; closes the dual-licence route unless a CLA is added later for future contributions only |

**Recommended default: C.** It is what the code already says, it needs no counsel-drafted texts, and it keeps apps free to use any licence. If you want the option to sell a commercial licence later, choose A before the first outside contribution, because a CLA cannot cover past ones.

## 7. Tests

- **Host tests** for every new crate and app logic (`kaituf`, `promptcore`, the `pkg` and `cc` services' logic, each app's model), with hostile tests through `testkit/hostile.rs` for each new parser (section 1).
- **Golden images** for each app's main view in each theme, rendered on the host through `uikit` and `gfx2d`, within per-image tolerances.
- **Per-app boot cases** (section 1), driven by the agent through the mock model keyed by request.
- **Tamper cases** for the image repository and the mirror: a flipped object byte, an expired timestamp, an older snapshot, an unknown key, a header that differs from its index entry (ADR 0011, S11-2).
- **Real-model runs** only on GitHub: S12-2, and fluency and actbench at task 15.
- **The release dry run** on a throwaway tag before any real one.
- Each run records its runner and run id in its PR.

## 8. Estimate

- **Core:** 20 PRs (task 16 is two). About 19,000 lines with tests (est.): packages 3,100 (tasks 1-3); apps 6,700 (tasks 4-7, 9, 10); themes 1,200; the editor 2,500; `c>`, `cc build` and model-written C 3,900; release work about 1,500 plus workflows. ADR 0011's 5,000-6,000 lines to v0.1 are partly built already in P1 (#100) and P2 (#143); ADR 0012's 5k-8k likewise (#78, #80).
- **New external crates**, all in ADR 0009's free class: `p256` 0.14.0 and `ecdsa` 0.17.0 (Apache-2.0 OR MIT), `ed25519-compact` 2.6.0 (MIT), `sha2` 0.11.1 in ring 3 (MIT OR Apache-2.0), wasm-encoder 0.262.0 and wasmparser 0.228.0 (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT), `olpc-cjson` 0.1.4 only if S08-3 needs it. Host-only tools: tuf-on-ci 0.21.0 (MIT) and `tough` 0.24.0 for tests. Each PR lists what it adds.
- **Timing:** the roadmap says months, overlapping P3. Once P3 task 9 merges, P4's length is set by task 1, the longest app task (the editor or `c>`), task 15 and the release: **about 9-13 weeks (est.)** from P4's entry to the `v0.1.0` tag. Of that, the spikes, task 16a, and the host halves of tasks 1, 11 and 12 (about 3-4 weeks of work) run before entry, beside P3. Your key ceremony and the name search can add 1-2 weeks at the end if they start late.

**Start with these three, in parallel, once the plan is approved:**

1. **S09-1 with S08-2, then task 16a** (session 2). GitHub only, and it touches no running lane's file.
2. **S12-2, then task 12's host half** (session 3). One runner job, then host tests.
3. **Task 1's host half** (session 1): the packer and the test-signed repository, host-tested. In the OS once P2 task 13 and `kaituf` land.

## 9. Risks

| Risk | What we do |
|---|---|
| P3's `uikit` is late | The spikes, the release work and the host halves of tasks 1, 11 and 12 run before it. Only the app tasks wait, and the v0.1 tag with them |
| `kaituf` is late (S08-3 or S11-1 fails) | ADR 0011's fallback: design C's line-format signed root, snapshot and timestamp, with everything else unchanged. Task 1 waits 1-2 weeks |
| `updated` and the update disk are late | Task 17's assets and signing do not need them; only `release-dry`'s update-disk step and the checklist line wait |
| S12-2 fails | `c>` ships for people only; task 14 drops. Nothing else moves |
| The pack runs over budget with seven app groups | S10-0's recount at task 15; the index level ships in v0.1 for the least-used groups (ADR 0010) |
| Qwen3-4B drives some app poorly | Declared actions first, `ui_view` then `ui_act` as fallback (P3); per-app actbench tasks measure it before the rows' wording freezes |
| An app's view runs over 300 tokens | S05-2's host gate on the real tree; the projection changes before the format freezes |
| A hostile package summary steers the AI | Index text is data (section 4, item 5); tiers are in code; `desk-catalog` tests an instruction-shaped summary |
| Hot-file waits on A-core and the model lane | Each turn is small and named in section 3.4; apps need no OS row of their own |
| You have not answered the licence, keys or name by the end | Release candidates keep coming as `rc` tags signed by the test root, and nothing reaches `stable` until the answers are in |
| Too many sessions at once | Three at approval and six at most, inside your 95% weekly cap; you set the number |
| GitHub Actions time | Real-model runs only for S12-2 and task 15; S09-1 under 30 minutes a run; KVM jobs gated as today |

## 10. Deferred, and where it goes

- **P5 (v0.2):** the online repository over TLS and internet `install`; the network grant replacing `pkgrepo=`; chunked resumable downloads; in-OS model download (L7); S11-3's GHCR mirror.
- **P6 (v0.2):** the `linux-layer` package and `apt install` into the guest; the browser.
- **P7 (v0.3):** the same image on a USB stick; S12-1's numbers re-measured on the founder's PC.
- **P8 (v1.0):** third-party publishers, hashed-bin delegations, a KMS for the online keys, Secure Boot, an SBOM, fuzzing `kaipkg`, `kaituf`, `promptcore`, `kailink` and `wasihost` at scale, a security review of the `cc` and `pkg` boundaries, and the native-code question (ADR 0012 D10).
- **Waiting on the founder:** section 2's questions, the licence first.
