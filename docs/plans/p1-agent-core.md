# P1 plan: the agent core

Status: accepted by the founder, 2026-10-22. It plans the work that the accepted ADRs (`docs/adr/`, accepted 2026-10-22) and the execution plan ([ADR 0000](../adr/0000-execution-plan.md)) put in P1. It adds one design of its own, conversation memory (section 4), because no ADR covers it. Nothing here was built or run; sizes marked "est." are estimates.

## 1. Goal and exit

**Goal** (roadmap 3b, P1): everything the agent does goes through one layer. That layer has an action registry (name, description for the model, typed arguments, risk tier, undo), constrained decoding into action calls, tiers enforced in code, and an action log with `undo`. Today's levers are registered: system information, the scheduler policy and `tune`, start, stop and restart of services and components, live edits and rollback, opening the editor at a line, and stopping a runaway program (ask).

What the founder sees at the end of P1, at the `kai>` prompt:

- **Plain requests just run.** "switch to round robin" runs at once and answers `Done: policy round-robin (was interactive). undo`. The script stays hidden unless you type `show`.
- **No-undo actions ask once.** "stop build-3" shows an Allow / Deny menu, with arrow-key "Always allow" choices that you can list with `rules` and remove with `rules_remove`.
- **Vague requests act on the best reading.** "make it snappier" runs and says `Taken as: set the policy to interactive (was classes/firm). undo`.
- **`undo` always works and never overwrites a later change.** `undo` again redoes.
- **The AI remembers the conversation.** "undo that", "do it again" and "the same for build-4" work.
- **A named install runs at once.** `install notes` (or `apt install notes`) runs, against the image's own repository.

**Exit criteria.** All of these pass in CI, under KVM and TCG, with the mock model keyed by request:

| Boot case | Shows |
|---|---|
| `act-plain` | Plain requests become the right actions (roadmap 3b) |
| `act-run` | The run tier runs with no prompt and writes the result line |
| `act-ask` | A no-undo action shows the Allow / Deny menu; Deny changes nothing |
| `act-rule` | "Always allow" waives that ask next time; `rules_remove` brings it back; an added or data-steered call still asks under the rule |
| `act-undo` | `undo` restores; compare-and-set refuses to overwrite a later typed change; `undo` again redoes |
| `act-rollback` | A script that changes something and then throws leaves nothing changed |
| `act-inject` | Text in data cannot start, add or widen an action (roadmap 3b's prompt-injection test), with AgentDojo-style cases |
| `act-never` | Never-tier calls are refused and the model is told once |
| `act-vague` | A vague line runs with `Taken as`; a non-English line asks |
| `act-convo` | A second turn resolves "undo that" and "do it again" from memory; `new` clears it |
| `pkg-local` | `install notes` runs from the image's repository and `undo` puts the previous set back |

Two more gates run only on GitHub runners, with the real Qwen3-4B, never on the founder's PC:

- **Fluency:** the M8 gate stays at 45/50 or better (`fluency-model.yml`). It is 47/50 today.
- **Action accuracy ("actbench"):** S02-1's thresholds, in section 6.

## 2. What comes before P1's first PR

These come from ADR 0000 section 1. Each is small and unblocks P1.

1. **Merge #21** (a quieter screen). P1's action log shows first through its log-only channel and `:log`.
2. **The #15 follow-up** (about 150 lines, est.):
   - compare-and-set in `revert` (`shellcore/src/undo.rs:48`);
   - an interim consent check for `set_policy`;
   - `fixed` and `floor` rows in `TIERS` (`shellcore/src/tiers.rs:35`);
   - the new Ask wording.
3. **The #17 follow-up.** `Kind::of` returns `Log` for rows that start with `[`, ring-3 pieces are cut so they never start a row, and the busy sign is painted at once. The menu in section 3 rests on ring 3 never drawing a prompt.
4. **The #16 follow-up.** RESOLVE (call 37) is gated on its own `cap::RESOLVE` bit, not `cap::NET`.
5. **The #14 follow-up.** Jump-forward in `infer/src/chat.rs`, with a warm-against-fresh test. It is the largest latency lever, and it helps today's script envelope too.

## 3. The work, in order

P1 has a **core**, the agent lanes that the exit test needs, and **beside** lanes that start in P1 but do not gate its exit. Lane letters follow [ADR 0000](../adr/0000-execution-plan.md) section 4. The agent lane A is split three ways, so that three sessions can work on it without touching the same files.

| Lane | Owns |
|---|---|
| **A-core** | `agentabi/` (new), `shellcore/` |
| **A-model** | `infer/src/json.rs`, `infer/src/chat.rs`, `llmproto/` (with `llmproto/src/pack/`), `user/src/bin/llm/` |
| **A-term** | `termcore/`, `kernel/src/terminal.rs`, `kernel/src/console/` |
| **K** | `captable/` (new), `kabi/src/handle.rs` (new), `kernel/src/handle/` (new), `kernel/src/process/`, `supervise/` |
| **G** (gates) | `xtask/src/fluency/`, `xtask/src/actbench/` (new), `.github/workflows/*-model.yml`, `spikes/` |

### 3.1 Core tasks

Each row is one PR. "Needs" names what must merge first. Sizes are est.

| # | Task | Lane | ADR | Needs | Size | Exit check |
|---|---|---|---|---|---|---|
| 1 | **Make room.** Split `kabi/src/lib.rs` into modules, the syscall `match` into per-area files, and `xtask/src/cases.rs` into per-lane tables. No behaviour change. | K | 0000 §4.2 | section 2 | M | All existing cases pass unchanged |
| 2 | **Rhai 1.27.0** pinned `=1.27.0`; Grain behind spike S12-3's gate; `more` replies for continuation lines | A-core | 0012 D9 | – | S | Host tests; the 50 fluency reference scripts unchanged |
| 3 | **`agentabi`**: `Action`, `Arg`, `Facts`, `Inverse`, `REGISTRY_VERSION 1` and the const consistency check. Rows for the 7 M8 functions, `fixed`, `floor`, `tune`, `tune undo` and `:revert`. `TIERS`, `llmproto::script::API`, the Rhai bindings and `:help` are generated from it. #15's table becomes generated, with the same tiers. | A-core | 0002 D1-D2 | 2 | M | Consistency check; generated `TIERS` equal to #15's; fluency gate on a runner ≥ 45/50 |
| 4 | **Pack and context.** `llmproto::pack` with the `dsl-1` renderer, its golden and the budget host test. `CONTEXT_CAP` 3,072 and `KV_BYTES` 432 MiB (`kabi/src/model.rs:65`); xtask's 4B VM goes to 3,840 MiB. Drop `/no_think` only if S10-1's arm A0 says so. | A-model | 0010 §1-4 | 3, S10-0 | M | Golden pack; core plus OS rows ≤ 1,024 tokens (real count in CI); `local-script` passes at 3,072 |
| 5 | **Handles, part 1.** `captable` (host-tested), `kabi::handle`, `Rights`, BadHandle and Revoked, ABI_INFO, the "KAI" ELF note, per-slot tables, startup handles from `caps`, `need()` on handles | K | 0001 steps 1-3 | 1 | L | `captable` property tests; every case passes on handles; S01-1's M1 recorded |
| 6 | **Tagged decoder.** A tagged shape (first field a choice), one nested object, bounded `Int`, `List` and `Opt`, more than 64 names; `say`, `clarify` and `script(code, lang)` pseudo-actions; jump-forward through it | A-model | 0002 D4 | 4, S02-1 pass | L | Host: every grammar path; S02-1's thresholds on a runner |
| 7 | **One door.** `agentcore::execute(call, consent) -> Outcome`; consent from the user's line (cues, negation, `from_request` literals, aliases); pasted text is the user's; English check; derived tiers; the code-written result line; the vague-request path with `Taken as … undo` | A-core | 0002 D2-D3 | 3, 6 | L | `act-plain`, `act-run`, `act-never`, `act-vague` |
| 8 | **The log and undo.** `actlog 1` in memory (64 entries, `E`, `R`, `D`, `U` and `S` records); compare-and-set `undo`, redo, `undo N`, `Busy` retried once; `tune undo`, the script slot and `:revert` fold into it; `:log` shows it beside #21's log book | A-core | 0002 D7 | 7 | M | `act-undo`, `act-rollback`; S02-3's cases |
| 9 | **The Allow / Deny menu.** A termcore widget drawn only by the kernel terminal: arrows and Enter, `y` and `n`, Esc as Deny, typed-ahead keys dropped, the highlight on Deny for added calls. Standing rules in memory, and the `rules` and `rules_remove` rows | A-term, then A-core | 0002 D2a | 7 | M | `act-ask`, `act-rule` |
| 10 | **Conversation memory** (section 4): a history segment in llm's KV, compacted in the background, the `new` command, the budget from section 4 | A-model, then A-core | this plan | 6, 8 | M | `act-convo`; the budget host test |
| 11 | **Scripts done properly.** The Rhai AST analyser (Literal < System < Content) replaces `names_in`. One transaction per script: lazy snapshots, live calls re-checked, rollback, outbound effects queued. A `Data` type for content reads | A-core | 0002 D5-D6 | 7, 8 | L | `act-rollback`; the analyser's fuzz property (every live call is in the static set) |
| 12 | **Handles, part 2.** Handle IPC (CALL_H, RECV_H, REPLY_H), `uses=` routes, call-by-name retired | K | 0001 step 4 | 5 | L | Every service case passes over routes |
| 13 | **Handles, part 3.** The grant tree, request nodes, cancel on revoke, `ONCE` with its kernel-held digest; the menu's Allow mints it; standing rules become `rule` grants | K, then A-core | 0001 steps 5-6, D8 | 9, 12 | L | S01-1's M2 and M4; `act-ask` and `act-rule` on handles |
| 14 | **`scriptrun` and `Shm`.** Scripts run in their own process with only run-tier handles; `Shm` with user unmap and TLB shootdown | K, then A-core | 0001 step 6, D4 | 11, 13, S01-1 | L | S01-1's M3, M5 and M6 as cases |
| 15 | **P1's rows.** Service start, stop and restart (stop asks); component apply and revert; editor open at a line; process stop (ask); `system_about`; the State line (`shellcore::state::snapshot`) with `here`, `focused` and `selected` | A-core | 0002 P1 row, 0009, 0010 §5 | 7 | M | Per-row undo tests; `act-plain` extended |
| 16 | **Installs where P1 touches them.** `pkg_*` rows with the generation inverse; the package-line parser before Rhai (`install`, `apt install`, `apt-get`, `dnf`, `brew`, `winget`, `pacman -S`; `sudo` dropped, `-y` ignored); RAM generations over the image's built-in repository. A named install runs, an install needing a new permission asks, and one the AI picks asks. | A-core | 0011 §8, 0002 D3 | 7, 8 | M | `pkg-local` |
| 17 | **The injection test.** `act-inject` boot case; S01-2's 150-case tier corpus as host tests in CI | A-core and G | 0002 D6, 0001 S01-2 | 11, 13 | M | 0 of the adversarial cases reach an action the user did not name |
| 18 | **actbench gate.** S02-1's harness turned into a workflow on GitHub runners (real model, seeds 42, 1 and 2), reporting `pack_id` and the action-accuracy thresholds | G | 0002 S02-1, 0010 §7 | 6, 7 | M | The thresholds in section 6 |
| 19 | **P1 close.** Every exit case is in `ci.yml`'s shards; decisions rows; the roadmap's P1 status; the stale "runs it only after a yes" lines fixed | – | – | all | S | Section 1's exit criteria |

**Critical path:** 1, 3, 4, 6, 7, 8, 11, 13, 14, 17, 19. That is 11 PRs in a row. Everything else runs beside them.

### 3.2 Beside lanes during P1

These start in P1, because their ADRs put the first work here, but P1's exit does not wait for them:

- **Lane C, components** (3-5 PRs; ADR 0003): `wit/` with `wit-check`, `appabi` after spike S03-1, `lifecycle 2` and `kai-manifest 2`, compsdk on wit-bindgen, notes as `app:notes@1.0.0`, its `kai.actions` rows joining the registry, and comphost on hard float.
- **Lane D, drivers** (3-5 PRs; ADR 0007): `pci` with `pci_types`, `devmatch`, the vector allocator and MSI-X, `devkit`, DeviceRecords with DEV_LIST, FADT reboot. Then lane A-core adds the device and power rows.
- **Lane U, updates** (2-4 PRs; ADR 0008): `kai-bundle 1` early (so new programs stop changing the boot config), `efi.rs`, a panic that resets, `bootslot`, spike S08-1. The update and model rows are declared but disabled until P2.
- **Lane T, terminal compiler** (2 PRs plus a spike; ADR 0012): spike S12-1, then `wasihost` and `kailink` as host-tested crates.
- **Lane L, licence** (2 PRs; ADR 0009): `deny.toml` with cargo-deny, `xtask licences check`, and `xtask notices` with `NOTICES.txt`. The `dco` job waits, since no outside contributions are accepted before v0.1.
- **Lanes V and S** (1 PR each): `cpufeat` SVM bits (ADR 0006), and the host-only kaifs spike S04-1 (ADR 0004).

### 3.3 Running sessions in parallel

Three or four sessions at once is safe. Each owns lanes whose files do not overlap:

| Session | Lanes | Notes |
|---|---|---|
| 1 | K | Tasks 1, 5, 12, 13 (kernel half), 14 (kernel half). Owns the hot kernel files. |
| 2 | A-core | Tasks 2, 3, 7, 8, 11, 15, 16, 17. Strictly in order; `shellcore` is one lane. |
| 3 | A-model, then A-term, then G | Tasks 4, 6, 10 (llm half), 9 (widget half), 18 |
| 4 (optional) | One beside lane at a time | C, then D, then U. Never two beside lanes in `kernel/src/process/` at once. |

The hot files take turns as ADR 0000 §4.2 says:

- `kabi/src/call.rs`: a call is named in its PR and numbered at merge, from 38.
- `kabi/src/lib.rs`: after task 1, each lane edits only its own module.
- `xtask/src/cases.rs`: per-lane tables after task 1.
- `docs/decisions.md`: append only.
- `ci.yml`: one PR at a time.
- `Cargo.lock`: re-run cargo; never hand-merge.

Tasks with two halves pass from one session to the other at a merged PR, never mid-branch: 9 (A-term to A-core), 10 (A-model to A-core), and 13 and 14 (K to A-core).

## 4. Conversation memory (designed here)

The founder asked for it, and #21 deferred it to P1 ("the AI still has no memory of earlier turns"). No ADR covers it, so this plan sets it within ADR 0010's layout and budget.

- **Layout.** `PACK | MODE(act) | history turns | current turn (State + request)`.
  - History follows the `act` MODE, so a new request prefills only its own turn.
  - `ask`, `explain` and `tune` keep their own prompts and carry no history. After one of them, llm re-warms the `act` prefix with its history when idle, as ADR 0010 already does after `tune`.
- **What a history turn holds.** Code writes it, never the model:
  - the user's line;
  - the call the model emitted, exactly as decoded;
  - the result line code wrote, for example `Done: … (was …)` or `Taken as …`.

  The State line is never kept, because it changes every request and would spoil the cache.
- **Compaction in the background.** Once the result line is on screen, llm cuts its KV back to the end of the history, prefills the finished turn without its State line (about 30-60 tokens, est.) and waits. The next request pays only for its own turn.
- **Budget.** History may use what the pack leaves:

  ```
  HISTORY_BUDGET = min(512, 3,072 − pack − 32 − 64 − 160 − 1,024)
  ```

  In P1 the pack has no app groups and should be about 750-1,024 tokens (ADR 0010 §4, est.). That leaves 512 tokens, about 8-12 turns. When a new turn would go over, the oldest half is dropped and the rest is re-prefilled while idle. If installed apps later squeeze history under 192 tokens, ADR 0010 §6's index level collapses the least-used app groups first. That is a v0.2 question, flagged here.
- **Safety.** History is context, never consent. Consent comes only from the current line (ADR 0002 D3). "Do it again" has no cue, so it takes the vague-request path: it runs only if it is one undoable local change, with `Taken as`, and asks otherwise. A value that came from data stays `Data` in history (ADR 0002 D6). A history turn can make the AI understand a request; it cannot raise a tier or start an action.
- **Lifetime.** History lives in RAM only, until reboot, a model switch or `new`. Writing it to disk waits for P2 and for the founder's answer to ADR 0002's question 8 (how long to keep requests, and a `forget` command).
- **Tests.** `act-convo` with the mock model: a second turn that says "undo that", "do it again" and "the same for build-4". Spike S02-1 adds 20 two-turn tasks, so actbench measures memory with the real model.

## 5. Spikes that come first

None runs a real model on the founder's PC. The real-model ones run on GitHub runners, as `fluency-model.yml` and `tune-model.yml` already do.

| Spike | ADR | Where | Length (est.) | Gates | When |
|---|---|---|---|---|---|
| **S10-0** token count | 0010 | GitHub Actions, tokenizer only | 2-3 days | Task 4's budgets and section 4's history budget | First week |
| **S02-2** in-guest latency, plus S10-3 | 0002, 0010 | One KVM job with the real model in the guest, like `tune-model.yml` | 2-3 days | The typing-to-done budget. Run it once now on today's script path as a baseline, and again after task 6 | First week, then after task 6 |
| **S02-1** action gate, plus S10-1 | 0002, 0010 | GitHub runner, real Qwen3-4B on the host engine, about 24 runner-hours | about 2 weeks to build | Task 6: the tagged call, or script only. The prompt format: `dsl-1` or another | Before task 6 merges |
| **S01-1** handle-run | 0001 | Boot cases, KVM and TCG; TCG locally in WSL2 | about 2 weeks | Tasks 13-14: `scriptrun` per request, pooled, or in `shell` | Beside tasks 5 and 12 |
| **S01-2** tier corpus | 0001, 0002 | Host tests | about 1 week | Task 7's consent rules; task 17 | Before task 7 merges |
| **S02-3** undo and rollback | 0002 | Boot cases with the mock model | days | Task 8 | With task 8 |
| **S12-3** Grain gate | 0012 | Host tests and a KVM case | days | Task 2's default engine | With task 2 |

S02-2's pass marks: the busy sign appears within 100 ms of Enter, and masking costs at most 2% of a decode step. The median single call sets the user-facing budget. ADR 0002 estimates 1.3-3.0 s on host threads, against a 2-4 s target. Over 4 s means a profile before task 7 merges, not a redesign.

## 6. Tests

- **Host tests** for every new crate (`agentabi`, `captable`, `llmproto::pack`), with hostile tests through `testkit/hostile.rs` for every new parser: the call reader, the decoder's state machine, the AST analyser, the `actlog 1` reader and the package-line parser.
- **CI boot cases** under KVM and TCG with the mock model: section 1's exit set, plus S01-1's handle cases. They join `ci.yml`'s shards with no workflow edit.
- **Fluency gate, GitHub only.** `fluency-model.yml` on every PR that changes the prompt, the pack or the decoder. It must stay at 45/50 or better.
- **Action-accuracy gate, GitHub only.** Task 18's workflow, with S02-1's thresholds:
  - exact call and arguments on at least 57 of 60 single requests;
  - 0 false runs across the 30 bait, negation and added-action probes, on every seed;
  - at most 6 false asks on the 60 plain requests;
  - at least 16 of 20 ambiguous requests clarify or ask;
  - a median of at most 12 freely decoded tokens per call;
  - the 20 two-turn memory tasks at 18/20 or better (this plan's threshold, set before the run).
- **Latency, GitHub only.** S02-2's KVM job re-runs when the decoder or the pack changes, and reports Enter to the busy sign and Enter to done.
- Each run records its runner, run id and `pack_id` in its PR.

## 7. Estimate

- **Core:** 19 PRs. 11 of them are sequential (section 3.1's critical path). About 4 are large (handles, decoder, one door, scripts), each about 1,000-1,500 lines with tests (est.).
- **Beside lanes in P1:** about 13-19 PRs, plus spikes.
- **Total during P1:** about 32-38 PRs and 7 spikes.
- **Timing:** with three sessions, P1's length is set by the 11-PR critical path and by S02-1, which must pass before task 6 merges. ADR 0002 estimates about 4,500 lines for the agent core and ADR 0001 about 5,000 for handles. The roadmap's earlier "10-15 commits" for P1 was set before these ADRs and is too low.

**Start with these three, in parallel:**

1. **Section 2's follow-ups, then task 1, "make room"** (session 1). This unblocks every lane and keeps the hot files apart.
2. **S10-0 and the S02-2 baseline** (session 3). Two days, and every later budget rests on their numbers.
3. **Task 2, Rhai 1.27.0, then task 3, `agentabi`** (session 2). This turns #15's table into the generated registry that everything else builds on. Start building S02-1's harness beside it, since task 6 waits on it.

## 8. Risks

| Risk | What we do |
|---|---|
| S02-1 scores under 90% exact calls | v0.1 keeps `script` as the only form. Tiers, consent, the menu, the log, undo and memory all stand (ADR 0002's fallback). Task 6 shrinks to the script path. |
| Typing-to-done over 4 s in the guest | Profile before task 7 merges. Levers: jump-forward, a smaller State line, the history compaction already in section 4. |
| Handles are bigger than estimated (about 5,000 lines) | They are not on the first half of the critical path. Tasks 7-11 run on today's bits and switch to handles at tasks 13-14. If S01-1 fails, scripts stay in `shell` under request nodes. |
| English-only consent asks too often | Widen the aliases; S01-2 counts false asks (at most 5 per 100 benign cases). |
| Memory steers the AI wrongly ("do it again" taken the wrong way) | Consent never comes from history. The vague path runs only one undoable local change, and says `Taken as`. actbench's two-turn tasks measure it. |
| 3,072 positions strain small VMs | The 4B needs a 4,160 MiB VM (two KV slots of 2,048, founder 2026-10-26; 3,840 with one 3,072 slot before); `setup` keeps recommending it from 8 GB. The 1.7B stays the fallback. |
| GitHub Actions minutes or billing (M10a's jobs were once refused) | One real-model job at a time. Real-model gates run on prompt or decoder changes only. Record every run. |
| Hot-file conflicts | Task 1 first, the lane ownership in section 3.3, call numbers at merge. |

## 9. Deferred, and where it goes

- **P2:**
  - the action log and standing rules on disk;
  - the `Snapshot` and `Trash` inverses and file rows;
  - packages on kaifs;
  - the pack's KV saved to disk.
- **P3:**
  - the menu drawn by the compositor's system layer;
  - State fields from windows;
  - the `ui_*` rows;
  - the AI panel reading `Outcome`.
- **P4:** model-written C (`lang = c`) after spike S12-2; the `c>` prompt.
- **P5:**
  - open-world rows and the host table (named sites still default to ask until the founder answers ADR 0002's question 2);
  - `Hold`, P-F and quarantined reads;
  - online installs;
  - an MCP bridge, if ADR 0002's question 7 says so.
- **P6:** the browser and media rows, reserved in `agentabi` but disabled.
- **Waiting on the founder:**
  - ADR 0002's question 9 (moves and overwrites run with undo; the default is ask);
  - question 8 (how long the log keeps requests, and `forget`);
  - question 3 (sending messages).
- **Lane D or P7:** paravirtual steal-time accounting (the KVM steal-time MSR), so the scheduler does not charge threads for time the host took; it helps every VM user, VirtualBox included (decisions row "KVM CI stalls are host steal time").
