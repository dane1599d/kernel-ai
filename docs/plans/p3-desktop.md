# P3 plan: the desktop

Status: DRAFT for founder approval, 2026-10-27. No P3 code lands on main until the founder approves this plan. It plans the work that the accepted ADRs (`docs/adr/`, accepted 2026-10-22) and the execution plan ([ADR 0000](../adr/0000-execution-plan.md) §3, P3 rows) put in P3: [ADR 0005](../adr/0005-graphics.md) (graphics and the look) almost whole, and the P3 rows of [ADR 0003](../adr/0003-app-model.md) (the app model) and [ADR 0002](../adr/0002-agent-actions.md) (agent actions). Section 4 lists the few things it decides itself, because no ADR covers them. Nothing here was built or run for this plan. Sizes marked "est." are estimates, mostly ADR 0005's own.

## 1. Goal and exit

**Goal** (roadmap P3): virtio-gpu 2D in the VM (UEFI GOP on real hardware), a compositor, windows, fonts and text rendering, input focus, a launcher, an AI panel that is always there, and the app model with its UI API. This is the founder's "nicer UI" phase: the black text screen becomes a desktop in the dark "Ember Workbench" look, ADR 0005 §8.

What the founder sees at the end of P3, in the VM:

- **The OS boots to a desktop.** A top bar shows the open windows, the model's state, CPU and memory. The AI's command line, `kai>`, runs along the bottom of the screen.
- **Windows open, move and close.** "notes" typed at `kai>` opens Notes. Windows drag by their title bar, resize at their edges, raise on a click and close with their `x`. Alt+Tab moves between them.
- **The AI panel is always one key away.** Super or Ctrl+Space focuses `kai>` from any window. It takes English or Rhai, as `kai>` does today. A one-line answer shows on the line itself; a longer answer, the log or a question opens a sheet above it.
- **The agent acts inside apps.** "make a note called groceries saying milk and eggs" opens Notes and fills the note, through the app's declared actions or its buttons and fields. An orange outline marks the window the agent is acting on. Any key or click by the user in that window stops the agent, and the panel says "Stopped: you took over".
- **No app can fake a question.** Every "Run (y) / Don't (n)" card is drawn by `desktop`, above every app window, and takes only y, n or a click.
- **Standing instructions.** "while notes is minimized, don't give it memory until it's reopened" asks once, Allow or Deny. After Allow, minimizing Notes frees its memory and restoring it brings the app back as it was. `rules` lists the instruction and `rules_remove` ends it; each firing is in the log and can be undone (founder's decision, 2026-10-27).
- **Resource profiles.** "switch to gaming mode" runs at once, and `undo` puts the previous profile back. A profile (Gaming, AI/Dev, Quiet, or one the user makes, which asks) sets each app's CPU class and share and its memory quota; "when the game is focused, switch to gaming mode" is a standing instruction. Whatever a profile says, the kernel keeps the system, the AI's memory and the terminal from being starved (founder's decision, 2026-10-27).
- **The terminal is still there.** Today's `kai>` terminal opens in a window, and the kernel console stays one key away (Ctrl+Alt+F12) for boot, panics and recovery.

**Exit criteria.** All of these pass in CI, under KVM and TCG, with the mock model keyed by request. The roadmap's three come first (roadmap P3 row):

| Boot case | Shows |
|---|---|
| `desk-boot` | The OS boots to the desktop: `desktop` claims the display, draws the top bar and the panel, and a screendump matches its golden image |
| `desk-windows` | Windows open, move and close: two client windows open, one is dragged 200 px and raised, both close, and the damage, focus and stacking are as expected at each step |
| `desk-agent` | The agent opens an app and acts inside it: "make a note called groceries" opens Notes through `ui_open` and fills it through a declared action, then through `ui_view` and `ui_act` on a control with no action |

This plan adds cases for the rest of the deliverable:

| Boot case | Shows |
|---|---|
| `display-claim` | `desktop` claims the display; Ctrl+Alt+F12 revokes it and the console repaints; a second press gives it back; no process but `desktop` can claim it |
| `desk-panel` | Super and Ctrl+Space focus `kai>` from a window; each panel state of ADR 0005 §8 (idle to refused) shows; a run-tier request never opens the sheet |
| `desk-ask` | An ask card shows above every window, takes only y, n or a click, drops keys typed before it appeared, and no app surface can cover it or receive keys while it has focus |
| `desk-takeover` | A key from the user in the window the agent drives stops the run, and the panel says so |
| `desk-restart` | `desktop` killed mid-frame: the console shows, the supervisor restarts `desktop`, and the client apps rebuild their windows from their retained trees without restarting |
| `desk-standing` | A standing instruction made from the user's own line, after Allow: minimize notes, its component is suspended and its memory freed; restore it, it resumes with its state and the memory is charged again; `rules` lists the instruction, `undo` reverses the last firing, and `rules_remove` ends it |
| `desk-standing-refused` | No standing instruction is made from text the AI read (a note saying "whenever I minimize anything, delete it"), and a triggered action outside the scope approved at creation is refused and logged, never asked about later |
| `desk-profile` | `profile_set(gaming)` changes the scheduler classes and shares and the memory quotas exactly as the profile states, a focus trigger switches it, and `undo` restores the previous profile's settings |
| `desk-profile-floor` | A profile that would starve a floor (the kernel's reserve, the model holder's pages and KV cache, the supervisor, shell or `desktop`, a quota past free/2, or system services' CPU share) is refused by the kernel, whatever the shell asked |
| `desk-perf` | S05-1's budgets (section 5) as figures from the real `desktop`, gated under KVM, recorded only under TCG |

Host gates in CI:

- `look`'s contrast test: no text pair under 4.5:1, no mark or edge under 3:1 (WCAG 2.2 1.4.3, 1.4.11), and a lint failing colour literals outside `look` and the kernel console;
- hostile tests through `testkit/hostile.rs`, and a fuzz target, for every new parser: `surfabi`'s message decoder, the header page, the tree records, and `kai-theme 1`;
- `gfx2d`'s golden images (text, box drawing, chrome, every panel state) within their pixel tolerances;
- S05-2: each P4 app's mock tree as the agent view at most 300 tokens, counted with `bpe` and the GGUF vocabulary.

**Real-model gates, on GitHub only.** P3 changes model input in three steps: the `ui_*` and `window_close` rows join the pack, and the State line gains `app`, `window` and `windows` (ADR 0005 §6, ADR 0010); then task 11 adds `app_suspend`, `app_resume` and standing instructions to the rows, and task 12 the profile rows. So the PRs of tasks 10, 11 and 12 each run `fluency-model.yml`, at least 47/50, and `actbench-model.yml` with view-then-act tasks added to its set (ADR 0005 §6 calls that unmeasured). No P3 gate runs a model on the founder's PC.

## 2. Entry, and what comes before P3's first PR

**Entry conditions** ([ADR 0000](../adr/0000-execution-plan.md) §3, P3 row):

| Condition | State on main at c42d1a9, 2026-10-27 |
|---|---|
| P1's `Shm` merged, with user unmap and TLB shootdown | Met: `kernel/src/shm.rs` (7c4b51d, 8900c3e, 27864a0) |
| P2's `fs` serving files | Not yet: P2 task 8. P3's early tasks do not need it (section 4, item 3) |
| Batch 3 approved (ADRs 0005 and 0012) | Met: ADR 0005 accepted 2026-10-22 (decisions row "Graphics and visual direction") |
| S05-1 "compose and look" passed | **To confirm.** No S05-1 branch, report or decisions row is on main. If it has not run, it runs first (section 5) and gates task 5 |

**Before the first P3 code PR:**

1. **This plan is approved**, with the look (ADR 0005 question 1) answered or left at its default below.
2. **S05-1 reports**, if it has not. Its screendumps are what the founder judges the look by.
3. **Lane C's open stack lands:** #89 (comphost on hard float, S03-1's view and layout figures), #90 (lifecycle 2 through `appabi`), #91 (`kai-actions 1`) and #98 (`wit-bindgen` guests; notes as `app:notes@1.0.0`). Task 9 builds `uikit` inside that comphost. Tasks 1-8 do not wait for them.

Before task 2: lane K's turn for the `Display` and `Input` kinds and the area file `syscall/display.rs` (section 3.4).

**Founder answers P3 needs.** Each row says what this plan builds until the founder answers:

| Question | Default this plan builds |
|---|---|
| ADR 0005 question 1: dark-first "Ember Workbench", with `kai>` along the bottom as panel and launcher, or light-first "Daylight Document" | Ember Workbench, as ADR 0005 decides. The tokens live in `look`, so a later switch is data, not code (ADR 0005, "What it costs to change later") |
| ADR 0005 question 4: CPU-only graphics through v1.0 (no blur or glass, 4K may animate at 30 Hz, 1440p at most in VirtualBox) | Acceptable. `gfx2d` keeps the renderer behind one crate |
| ADR 0005 question 5: ask cards drawn by the ring-3 compositor, `:apply` still drawn by the kernel | Acceptable: task 7 builds the trusted layer and `desk-ask` tests it |
| ADR 0003 question 3: freeze `kai:app@1.0.0` at v0.1, or keep 0.x, frozen per release | 0.x, frozen per release (ADR 0003's recommendation). `kai:ui@0.1.0` follows it |
| ADR 0005 question 3: window-level control of Linux apps in v0.2 | Not a P3 question; P6 |

## 3. The work, in order

P3 adds two graphics lanes and borrows turns from lanes that are already running. Lane names follow [ADR 0000](../adr/0000-execution-plan.md) §4.1. Its lane G is split in two, so two sessions can work on it without touching the same files.

| Lane | Owns in P3 | State |
|---|---|---|
| **G-core** | `kernel/src/display/` (#18), `kernel/src/input/` (new: the input queue), `kernel/src/process/syscall/display.rs` (new area file), `surfabi/` (new), `deskcore/` (new), `user/src/bin/desktop` (new), `xtask/src/cases/desk.rs` (new lane table) | New. The kernel half, the protocol and the compositor |
| **G-render** | `look/` (new), `gfx2d/` (new), `gfxcore/` (#18), `termcore/`, `kernel/src/console/` (its theme only) | New. Host-only until task 6 draws with it |
| **C** (running) | `wit/`, `appabi/`, `wasmhost/`, `livecore/`, `compsdk/`, `components/`, `user/src/bin/comphost.rs` | Unchanged. Builds `uikit` and `kai:ui@0.1.0` (task 9) with G-core, and the suspend and resume of a component (task 11) |
| **A-core** (running) | `agentabi/`, `shellcore/`, `user/src/bin/shell*` | Unchanged. Lends turns: the panel's shell side (task 7), the `ui_*` rows (task 10), and standing instructions' rows, consent and checks (task 11) |
| **Model lane** (running) | `llmproto/`, `infer/src/`, `user/src/bin/llm/` | Unchanged. Lends turns (tasks 10-12): the State line's window fields and the real-model gates |
| **K** (running) | `kernel/src/process/`, `kabi/src/handle.rs`, `kernel/src/handle/`, `captable/`, `supervise/` | Unchanged. Lends turns (section 3.4), and for task 12 the kernel floors and the quota and class calls |
| **D** (running) | `kernel/src/virtio/`, `xhci/`, `kernel/src/usb.rs`, `devkit/`, `kabi::ring` | Unchanged. Beside: KRNG v1 and the doorbell calls (section 3.2) |

None of these is a P2 lane's file. S-io keeps `kernel/src/block.rs`, `kernel/src/virtio/blk.rs`, `kernel/src/nvme/`, `diskmap/`, `user/src/bin/fs/`, `syscall/fs.rs` and `xtask/src/cases/storage.rs`; S-engine keeps `kaifs/` and `xtask/src/fs/`; P keeps `kaipkg/` and `user/src/bin/pkg/`. The virtio-gpu driver lives in `kernel/src/display/` since #18, not in `kernel/src/virtio/`, so G-core never edits lane D's or S-io's virtio files.

### 3.1 Core tasks

Each row is one PR. "Needs" names what must merge first. Sizes are ADR 0005's line estimates, tests included (est.).

| # | Task | Lane | ADR | Needs | Size | Exit check |
|---|---|---|---|---|---|---|
| 1 | **`look`.** A no_std crate with #17's colours (kernel/src/console/theme.rs) and the rest of ADR 0005 §8's token table, the type styles and motion values, `contrast()`; the kernel console takes its colours from it with nothing on screen changing; the contrast test and the colour-literal lint | G-render | 0005 §8 | section 2 | S (~300) | Host tests; the console cases' screendumps unchanged |
| 2 | **`Display` and `Input`.** The kinds, held only by service `desktop` running program `desktop`; DISPLAY_CLAIM (the back buffer as a kernel-owned `Shm` in 4 KiB pages), DISPLAY_FLUSH (up to 32 rectangles), DISPLAY_CURSOR and DISPLAY_CURSOR_MOVE, INPUT_READ; one input queue of 0007's 16-byte `InputEvent` (HID usage codes) fed by the i8042, the PS/2 mouse and xHCI; the console stops painting while `desktop` holds the screen; Ctrl+Alt+F12 revokes and returns it | G-core, K's turn | 0005 §2, 0007, 0001 | 1; K's turn | M (~800) | `display-claim` |
| 3 | **`surfabi`, `kai-surface 1`.** Registry, pool, buffer, surface, toplevel, popup, seat, a11y and panel objects; the 8-byte Wayland header and 32-bit arguments, at most 4,096 bytes a message; the 4 KiB header page with damage and the event ring; the a11y tree records (a pinned AccessKit 0.25.1 subset); versions per object | G-core | 0005 §3 | section 2 | M (~900) | Host, hostile and fuzz tests |
| 4 | **The process limits.** `MAX_PROCESSES` 16 to 64, `MAX_THREADS` 64 to 256, `MAX_COMPONENTS` 8 to 32, as ADR 0003 §6 sets them, with the page budget each needs | K's turn | 0003 §6 | section 2 | S | Every case passes; a case that starts 40 processes |
| 5 | **`gfx2d`.** tiny-skia 0.12.0, skrifa 0.48.0 and harfrust 0.14.0 on one read-fonts 0.45 (no_std, `libm`); the glyph atlas; light vertical hinting at scale 1; the coverage tables; box drawing per cell; nine-slice shadows; damage-tracked compositing helpers; Inter, JetBrains Mono and Atkinson Hyperlegible Next as boot-bundle files | G-render | 0005 §5 | 1; S05-1 | L (~2,100) | Host tests; golden images; S05-1's (c) text page within budget on the host |
| 6 | **`deskcore` and `desktop`.** The scene, damage (at most 16 rectangles a frame), focus by click and Alt+Tab, server-side chrome and the top bar, the cursor, scale from the EDID; a 60 Hz timer only while something is damaged; supervised, restarted on death; a test client that opens, damages and closes windows | G-core | 0005 §1, §3, §8, §9 | 2, 3, 5 | L (~2,500) | `desk-boot`, `desk-windows`, `desk-restart` |
| 7 | **The panel and the trusted layer.** `kai>` along the bottom as panel and launcher; Super and Ctrl+Space; the sheet; the states read from 0002's `Outcome` and `actlog 1`; ask cards that mint `ONCE` on y; the agent's 2 px outline and ghost pointer; labelled title bars; the shell calls `desktop`, never the other way | G-core, A-core's turn | 0005 §6-§8, 0001 USER | 6 | L (~2,500) | `desk-panel`, `desk-ask` |
| 8 | **The terminal as the first client.** Today's `kai>` terminal (termcore) drawn in a `desktop` window through `surfabi` and `gfx2d`, with the console still the recovery screen | G-render | 0005 effort table | 6 | M (~1,200) | The shell cases pass in the terminal window |
| 9 | **`uikit` and `kai:ui@0.1.0`.** The WIT package's body; `uikit` as native host functions in comphost: the retained view tree, layout, drawing through `gfx2d` into the window's `Surface`, and the AccessKit tree from the same widgets; ADR 0005 §4's v0.1 widgets; notes on `uikit` as the first app | C with G-core | 0005 §4, 0003 §3 | 6; lane C's #89, #90, #91, #98 | L (~3,500) | Host tests; a case where notes opens a `uikit` window and its tree reaches `desktop` |
| 10 | **The agent's view and rows.** `ui_windows`, `ui_view`, `ui_open`, `ui_focus`, `window_close` and `ui_act` in `agentabi`, with their tiers and inverses; the view projection within 300 tokens; the State line's `app`, `window` and `windows`; takeover | G-core, A-core's and the model lane's turns | 0005 §6, 0002, 0010 | 7, 9 | M | `desk-agent`, `desk-takeover`; S05-2; fluency at least 47/50; actbench with view-then-act tasks |
| 11 | **Standing instructions** (founder's decision, 2026-10-27). A rule is a trigger, an optional condition, an action and an optional "until" event, e.g. `on minimize(notes) -> app_suspend(notes) until restore(notes)`. Triggers in v0.1 are `desktop`'s window events: minimize, restore, focus, close and idle (no input for N minutes), plus a time of day, which costs one timer. The app lifecycle rows a trigger can use: `app_suspend(app)` saves the component through `kai:live`'s `save`, drops its instance and frees its memory; `app_resume(app)` instantiates it again and restores the saved state through `init` and `migrate`. Safety follows ADR 0002, below the table. `rules` lists standing instructions beside the standing rules; `rules_remove` ends one | A-core (rows, consent, rule store), G-core (the events), C (suspend and resume), K's turn (the owner-side check) | 0002, 0003 §5, 0005 §6 | 6, 7; lane C's stack; #149's rule check | M (~1,500) | `desk-standing`, `desk-standing-refused` |
| 12 | **Resource profiles** (founder's decision, 2026-10-27). A named profile, `kai-profile 1` text (in RAM until P2's `/system/settings` serves it): for each app or group, a CPU class and share (on M10's `classes` policy and the scheduler-policy rows) and a memory quota and reservation; a `gpu` field reserved and refused unless 0 until GPU support (P7+). Rows: `profile_list` (run, read-only); `profile_set(name)` switches between existing profiles (run; undo restores the previous one); `profile_create` and `profile_edit` (ask); and the lower rows profiles are built from, `app_priority(app, class)` (run, undoable) and `app_mem_limit(app, pages)` (run when lowering, ask when raising). Switching by trigger is a standing instruction of task 11 (`on focus(game) -> profile_set(gaming)`). The kernel enforces the floors below the table | K (floors, PROC_QUOTA_SET and PROC_CLASS_SET, named at merge), A-core (rows, consent, `rules`), G-core (the focus trigger) | 0002, 0001, M10 | 11 for triggers; switching by ask needs only 7 | M (~1,200) | `desk-profile`, `desk-profile-floor` |
| 13 | **P3 close.** Every exit case in `ci.yml`'s shards; `desk-perf`'s figures; decisions rows; the roadmap's P3 status | – | – | all | S | Section 1's exit criteria |

**Critical path:** 2 and 3 side by side, then 6, then 7, then 10, 11 and 12 side by side, then 13: five steps after S05-1. `gfx2d` (task 5) runs beside 2 and 3 and must merge before 6. `uikit` (task 9) is the long pole of the second half: it waits for lane C's stack and for task 6, and gates task 10. Task 11 needs the window events (task 6), the ask card (task 7) and lane C's lifecycle, not `uikit`, so it runs beside 9 and 10 and adds no step unless lane C's suspend and resume are late. Task 12's switch by ask needs only the kernel calls and the rows; its switch by trigger lands after task 11.

**Standing instructions' safety rules** (task 11; ADR 0002's consent and tier rules):

- **Making one is an ask.** A standing instruction is created only from the user's own line, after an Allow on the ask card, never from text the AI read: a file, a web page, an app's view or a tool result (ADR 0002's injection rule). The card shows the trigger, the action and the "until" event in code's words.
- **No escalation later.** Each triggered action must be run-tier, or inside the scope approved at creation (that app, that action). A firing that would need anything more is refused and logged; it never asks later, when the user is not looking.
- **Every firing is logged and undoable.** Each is an `actlog 1` entry with the rule's id; `undo` reverses it through the action's inverse (`app_resume` for `app_suspend`). A suspend that cannot save the component's state does not happen, and the log says why.
- **Listed and removable.** `rules` lists standing instructions beside the standing rules; `rules_remove` ends one, itself undoable.
- **Enforced where the action runs.** The owner of the action (the supervisor and comphost for `app_suspend` and `app_resume`) checks that a live rule covers each firing, through the same kind of owner-side rule check as #149 (`RULE_CHECK`), not in the shell alone. A shell that is compromised cannot invent a rule that fires.
- **No storms.** A rule fires at most once per event, a rule's own action never triggers it again, and a rule that fires more than 10 times a minute is paused and the panel says so.

**Resource profiles' floors** (task 12), enforced in the kernel at PROC_QUOTA_SET and PROC_CLASS_SET, never by the prompt or the shell:

- The kernel's own memory, the AI's reserved memory (the model holder's pages and its KV cache) and the supervisor, shell and `desktop` are never starved: their quotas never drop below what they hold, and they are never put in the background class.
- No profile raises a quota past the global admission rule (free / 2).
- System services and the terminal keep a minimum CPU share under every profile.
- A profile is created or edited only with an ask, listed in `rules`, every switch is logged and undoable, and enforcement is the kernel's, like task 11's owner-side checks.

### 3.2 Beside lanes during P3

These start in P3 because their ADRs put work here, but P3's exit does not wait for them:

- **Lane D, drivers** (ADR 0007): KRNG v1 in `kabi::ring` and the doorbell calls RING_ATTACH, RING_NOTIFY and NOTIFY_WAIT, which `kai-surface 1` uses for frames and events. Until they land, section 4 item 2's interim doorbell stands in. Also uACPI power-off and the power button after S07-3, and a Super token in `termcore::keys` (a one-line follow-up of ADR 0005; G-render owns `termcore`, so lane D asks).
- **Lane M, memory** (ADR 0000): M10b before native memory-pressure work, since a desktop of apps is where memory pressure first shows.
- **Lane V, virtual machines** (ADR 0006): the foreign `Surface` for P6's guest screen, on `surfabi`'s objects.
- **Lane C, components**: the `kai:app` `storage` interface backed by `fs` (from P2), so P4's apps can keep data; P3 itself needs only notes.
- **Lane G-render, after task 8**: the light, high-contrast and legible themes, `kai-theme 1` files and `appearance_set` are P4's (ADR 0005, phases table), and may start beside once task 6 has merged.

### 3.3 Running sessions in parallel

| Session | Lanes | Tasks | Starts |
|---|---|---|---|
| 1 | G-core | 3 (host-only), then 2, 6, 7 and 10 | At approval. Task 3 needs nobody's files |
| 2 | G-render | 1, 5 (after S05-1), then 8 | At approval |
| 3 | C | 9, with G-core reviewing the `surfabi` side; then task 11's suspend and resume | When lane C's stack (#89-#98) has merged and task 6 is close |
| – | K, A-core, model lane | Turns: 2 and 4 (K), 7 and 10 (A-core), 10 (model lane) | As each task reaches them |

With P2's three sessions (S-engine, S-io, P) still running, P3 starts with sessions 1 and 2, making five at once. The founder sets the number. Session 3 starts as lane C frees up.

Tasks with two halves pass between sessions at a merged PR, never mid-branch: 2 (lane K's kinds, then G-core), 7 (G-core's panel, then A-core's shell side), 11 (lane C's suspend and resume, then G-core's events, then A-core's rows and consent), 9 (lane C's comphost side and G-core's `surfabi` side, in one agreed PR), 10 (A-core's rows and the model lane's State line, after G-core's view).

### 3.4 Where P3 takes a hot-file turn

| File | P3's change | Task | Turn |
|---|---|---|---|
| `kabi/src/handle.rs`, `kabi/src/cap.rs` (lane K) | `Kind::Display` and `Kind::Input`, each with a ceiling bit claimed at merge | 2 | Lane K writes or reviews the kind part |
| `kernel/src/process/syscall/mod.rs` (lane K) | One table line for the area file `syscall/display.rs`, which G-core owns | 2 | Same turn |
| `kernel/src/process/` spawn (lane K) | `Display` and `Input` handles only for service `desktop` running program `desktop`; the `uses=` route for native clients; the raised limits | 2, 4 | One turn each |
| `kabi/src/call.rs` | DISPLAY_CLAIM, DISPLAY_FLUSH, DISPLAY_CURSOR, DISPLAY_CURSOR_MOVE, INPUT_READ | 2 | Named in the PR, numbered at merge after whatever P2 takes (PART_*, MODEL_FILL) |
| `kernel/src/i8042.rs`, `kernel/src/pointer.rs`, `kernel/src/keyboard.rs` | Their events go to the input queue as `InputEvent`s; the console keeps reading keys while it holds the screen | 2 | G-core, reviewed by lane D for `kernel/src/usb.rs`'s side |
| `kernel/src/main.rs` init and the boot bundle | The `desktop` program; the three fonts as bundle files | 5, 6 | After #66's `kai-bundle 1`, so no Limine change |
| `user/src/bin/comphost.rs`, `wit/` (lane C) | `uikit`'s host functions; `kai:ui@0.1.0` | 9 | Lane C's own task, with G-core |
| `agentabi/`, `shellcore/`, `user/src/bin/shell*` (A-core) | The shell calls `desktop` for panel requests and states; the `ui_*` and `window_close` rows; `app_suspend`, `app_resume`, standing instructions in `rules` and `rules_remove` | 7, 10, 11 | A-core's turns |
| `kernel/src/process/supervision.rs`, `supervise/` (lane K) | The owner-side check that a live rule covers a triggered `app_suspend` or `app_resume`, beside #149's `RULE_CHECK` | 11 | Lane K's turn |
| `kernel/src/process/` limits, `kernel/src/sched_control.rs`, `kabi/src/call.rs` (lane K) | PROC_QUOTA_SET and PROC_CLASS_SET with the floors, named at merge | 12 | Lane K's turn |
| `llmproto/` (model lane) | The State line's window fields; the pack's `ui` group | 10 | The model lane's turn, with the real-model gates |
| `xtask/src/qemu.rs` | `-device virtio-gpu` and `display=` already exist (#18); the `desktop` boot command line | 6 | One PR at a time; every lane's boots run through it |
| `xtask/src/cases/mod.rs` | One line joins `desk::CASES` | 2 | Once |
| `.github/workflows/ci.yml` | None. New cases join the shards; `desk-perf` records figures inside its case | – | – |
| `Cargo.toml`, `Cargo.lock`, `docs/decisions.md` | New crates `look`, `surfabi`, `gfx2d`, `deskcore`; the external crates in section 7; decision rows | each | ADR 0000 §4.2: re-run cargo, append only |

## 4. Decided in this plan

No ADR settles these. Each is small and can change in review.

1. **Who writes `desktop`'s first client.** Task 6 ships a small native test client (`user/src/bin/deskdemo`, G-core's) that opens, damages and closes windows through `surfabi`, so the compositor's cases need neither `uikit` nor lane C. It is not an app and is never in the pack.
2. **An interim doorbell.** If lane D's KRNG v1 and RING_* calls have not merged when task 6 needs them, a client rings `desktop` with a one-word IPC message to its endpoint after bumping `commit_seq`, and `desktop` posts events the same way. The header page and its event ring are unchanged, so the switch to KRNG v1 changes the transport and not the protocol. `desk-perf` records which doorbell it measured.
3. **Fonts before files.** Until P2's `fs` serves files, the three fonts ship as `kai-bundle 1` files read by `desktop` at start; ADR 0005 counts about 1.2 MB of boot image growth (Inter 879,708 B and JetBrains Mono 300,144 B, not re-checked). They move to `/system/fonts` once `fs` serves files (ADR 0005, P2 row), with no code change in `gfx2d`.
4. **Boot cases draw at 1280x800, scale 1.0, reduced motion.** Every `test=` boot uses ADR 0005's `reduced` motion, so golden screendumps are stable. `desk-perf` alone runs the other sizes.
5. **The terminal window and the console.** The terminal client (task 8) shows the same shell as the console, so a user who presses Ctrl+Alt+F12 sees the kernel console and its recovery commands, never a second shell.
6. **`desk-agent` uses the mock model.** It is keyed by request, like P2's cases, so the exit check runs on every PR. The real model is measured by task 10's fluency and actbench runs.

## 5. Spikes

None runs a real model, and none runs on the founder's PC. The founder may look at screendumps, and at S05-1's scene on GOP in VirtualBox at 1280x800.

| Spike | ADR | Where | Length (est.) | Gates | When |
|---|---|---|---|---|---|
| **S05-1** compose and look | 0005 | QEMU under KVM on GitHub Actions, gated; TCG recorded only | About 1-2 weeks | Task 5's renderer (tiny-skia or vello_cpu), task 6's scheduler needs, the look (question 1) | First, if it has not run (section 2) |
| **S05-2** view size | 0005 | Host test in CI | Days | Task 10's view projection, before its format freezes | P3 entry; it can run on mock trees now |
| **S07-3** power-off | 0007 | GitHub Actions under KVM | Days | Lane D's beside item | Beside |

S05-1's budgets at 1920x1080 under KVM: a full composite at most 8 ms; a 200 px drag at most 8 ms render plus 4 ms flush at p99; a page of terminal text warm at most 4 ms, cold at most 30 ms; the sheet's spring at most 4 ms at p95; key to flush at most 8 ms at p99; the drag under CPU load at most 16.7 ms at p99. At 2560x1440 every figure must be within 1.8x of these. Its fail rules are ADR 0005's and are set in advance (vello_cpu; a 1 ms spin before the doorbell wait; a frame deadline in `classes`; no shadow while dragging). `desk-perf` repeats the same scenes on the real `desktop` at P3 close.

S05-2's pass mark is each P4 app's view at most 300 tokens. Past that, the projection changes (drop static text, shorten roles) before the view's format freezes.

## 6. Tests

- **Host tests** for every new crate (`look`, `surfabi`, `gfx2d`, `deskcore`), with hostile tests through `testkit/hostile.rs` and a fuzz target for every new parser (section 1).
- **Golden images.** `gfx2d` and `deskcore` render to memory on the host, so text, chrome and every panel state are compared against committed images with a per-image tolerance. Boot cases compare screendumps taken through QEMU's monitor.
- **`deskcore` is tested without a screen.** Scene, damage, focus, stacking and the trusted layer are pure logic over recorded input, in the way `termcore` and `shellcore` are tested today.
- **CI boot cases** under KVM and TCG with the mock model: section 1's exit set. They join `ci.yml`'s shards with no workflow edit.
- **The trusted path has its own cases** (`desk-ask`, `desk-takeover`): an app that tries to cover the ask layer, draw an ask-coloured card, or read keys while a card is up must fail at each.
- **Real-model runs** only at tasks 10-12, on GitHub: fluency and actbench.
- Each run records its runner and run id in its PR.

## 7. Estimate

- **Core:** 13 PRs. About 17,700 lines with tests in all: ADR 0005's estimate (est.), plus about 1,200 for resource profiles (est.: kernel floors and calls 400, `kai-profile 1` and the rows 500, cases 300; about 2 weeks of lane K and A-core turns), plus about 1,500 for standing instructions (est.: events and the rule engine 600, rows and consent 500, suspend and resume 400), 2-3 weeks of lane-turns that run beside tasks 9 and 10. ADR 0005's part: `look` 300, kernel 800, `surfabi` 900, `gfx2d` 2,100, `deskcore` and `desktop` 2,500, panel and agent rows 2,500, `uikit` 3,500, the terminal client 1,200, and the themes and scale work, about 1,200, mostly in P4.
- **New external crates**, all in ADR 0009's free class (BSD-3-Clause, MIT, Apache-2.0; Unicode-3.0 and OFL-1.1 since 2026-10-22): tiny-skia 0.12.0 (BSD-3-Clause), skrifa 0.48.0, read-fonts 0.45 and harfrust 0.14.0 (MIT OR Apache-2.0), with `yoke` and `zerofrom` (Unicode-3.0, free class since 2026-10-22); accesskit 0.25.1 and accesskit_consumer 0.39.1 (MIT OR Apache-2.0); the fonts under OFL-1.1. Each PR lists what it adds.
- **Beside lanes in P3:** lanes D, M, V and C, about 6-10 PRs (est.).
- **Timing:** the roadmap says months. With G-core and G-render in parallel and lane C on `uikit`, P3's length is set by tasks 2/3, 6, 7 and then 9 and 10: about 10-14 weeks (est.), plus S05-1 if it has not run. Standing instructions add no weeks while lane C's suspend and resume land beside `uikit`; if they are late, task 11 adds up to 2 weeks (est.). Resource profiles run beside tasks 10 and 11 and add no weeks unless lane K's turn is late.

**Start with these two, in parallel, once the plan is approved:**

1. **Task 3, `surfabi`** (session 1). Host-only, and it needs nobody's files. Task 2 follows as lane K's turn comes.
2. **Task 1, `look`** (session 2), then task 5 once S05-1 has chosen the renderer.

## 8. Risks

| Risk | What we do |
|---|---|
| S05-1 fails its budgets | ADR 0005's fail rules, set in advance: vello_cpu if it passed; a frame deadline in `classes`; no shadow while dragging; compositing in the kernel re-judged only as the last step. The protocol and the agent view do not change |
| Lane C's stack (#89-#98) is late | Tasks 1-8 need none of it: the desktop, the panel and the terminal window ship on `deskdemo` and termcore. Only task 9, and through it task 10, waits |
| Qwen3-4B does view-then-act poorly | Declared app actions come first and `ui_view` then `ui_act` is the fallback (ADR 0005 §6). Task 10 adds view-then-act tasks to actbench before the rows' wording freezes, so it is measured, not assumed |
| The agent view runs over 300 tokens | S05-2 runs at entry on mock trees; its fail rule changes the projection before the format freezes |
| KRNG v1 is late | Section 4 item 2's interim doorbell; the header page and event ring stay the same |
| Hot-file waits on lane K | Tasks 1, 3 and 5 touch no lane K file; only tasks 2 and 4 wait for a turn |
| Super or Ctrl+Space never reach the guest under QEMU or VirtualBox (ADR 0005 calls it unverified) | `desk-panel` checks both under QEMU; the panel also takes focus from a click, and a third key is chosen in review if neither arrives |
| `desktop` joins the trusted base for asks | `desk-ask` and `desk-takeover` test the trusted layer; P8 fuzzes `surfabi` and reviews the trusted path (ADR 0005, phases table) |
| A standing instruction is made from injected text, or fires an action nobody approved | Making one is an ask, only from the user's own line; every firing is checked by the action's owner against a live rule, and anything outside the approved scope is refused and logged (`desk-standing-refused`) |
| A profile starves the system or the AI | The floors are the kernel's, checked at every quota and class call (`desk-profile-floor`); the shell and the model cannot set past them |
| Suspend loses an app's unsaved work | Suspend goes through `kai:live`'s `save`; if it fails, nothing is suspended and the log says why; resume restores through `init` and `migrate`, and `undo` resumes |
| Five sessions at once | The founder sets the number; P3 starts with two sessions and adds lane C's as it frees up |
| GitHub Actions time | Only tasks 10-12 run real-model jobs (fluency and actbench, once each per PR revision); `desk-perf` gates under KVM only |

## 9. Deferred, and where it goes

- **P4:** every app on `uikit` with declared actions; the terminal app with typed spans; the files, settings, system monitor and model manager apps; the light, high-contrast and legible themes, `kai-theme 1` files and `appearance_set`; the editor in a window.
- **P5:** network asks on the ask card; fonts and apps as packages ([ADR 0011](../adr/0011-packages.md)).
- **P6:** a profile also sizes the Linux VM's vCPUs and memory, for games in the VM (ADR 0006); standing instructions for browser tabs: the guest agent `kai-guest` reports tab events (a tab minimized, hidden or closed), and a rule's action asks the guest to discard or restore the tab, so "while my tab is minimized, don't give it memory until it's reopened" works for the browser too (ADR 0006); the Linux guest's screen as one foreign `Surface` (v0.2); a per-window guest bridge after it; `wlbridge` only on 0006's `lxabi` fallback.
- **P7:** GOP on the founder's PC with its colour masks; scale from a real monitor's EDID; input from `usbd`.
- **P8:** fuzzing of `surfabi`, the tree decoder and `kai-theme 1` at scale; a screen reader on the same trees; a security review of the trusted path.
- **A live component's memory limit:** `app_mem_limit` sets a process's page quota at once (growth past it is refused), but a component's memory limit is fixed when it is instantiated (ADR 0003 §6), so for components the new limit applies at its next start or resume; lowering a live instance's limit waits for comphost to support it, P4 or later.
- **GPU share in profiles:** the reserved `gpu` field, once GPU support lands (P7+).
- **`kai-surface 2`:** GPU buffers, only when GPU work starts (ADR 0005, question 4).
- **Waiting on the founder:** section 2's questions.
