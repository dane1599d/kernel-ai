# Agents OS plan: wiring ADR 0013 into the OS

Status: a work map, 2026-11-18. The founder lifted the v0.1-tag gate on agent features that day (v0.1 ships from `release/v0.1`), so the OS half of [ADR 0013](../adr/0013-agents.md) lands on main now. This plan adds no product decision. It cuts what ADR 0013 already approved (its "after the v0.1 tag" phase, about 11,000 lines est., plus P5 task 14's ~500) into one PR per row, in [p1-agent-core.md](p1-agent-core.md)'s format. Anything that needed a new product answer went to the manager as a founder question (section 6) and is not decided here.

Sizes are estimates (est.). "Needs" names rows or PRs that must merge first. "Gate" is what must pass beyond the PR's own host tests and lint; "fluency" means `fluency-model.yml` at 47/50 or better and "actbench" S02-1's thresholds on seeds 42, 1 and 2, both on GitHub runners only. A boot case runs in `ci.yml`'s shards under KVM and TCG with the mock model, in the owning lane's table in `xtask/src/cases/`.

## 1. What is already on main

Every ADR 0013 host crate, each `no_std` and host-tested, wired into nothing: `agentsched` (the scheduler's policy, with `tests/engine.rs` driving the real engine exactly), `agentid` (roster, badges, the door's admission, asks, `actlog 2` stamps), `kaimem` (`kai-memory 1`, the store, who may remember, the directive rule, the prompt section), `goalcore` (goal records, steps, check-ins, `kai-goals 1`), `kaiagent` (an agent package's manifest and budgets), `semindex` (`kai-index 1`, chunks, BM25 plus cosine, `since`), and P5's `netdest` and `nethosts`. The reservations are in: `kind agent` refused by name in `kaipkg` format 1, the `agent`, `goal`, `memory` and `search` row groups taken in `agentabi::names`, and the `memory` and `index` volume names reserved in `kaifs::vol`. The spikes are done: S13-1 passed its two measured bars, S13-2 failed (decode stays serial), S13-3 and S13-4 and S13-5 passed.

## 2. The rows

### 2.1 Foundation

| # | Task | Lane | Needs | Gate | Boot case | Size |
|---|---|---|---|---|---|---|
| F1 | **`actlog 2`.** `E` records carry `agent=<name>` from `agentid::stamp`; v1 records read as `kai`; `undo <agent>` undoes that agent's last entry; `:why` and `:log` show the name | A-core | – | host: v1 and v2 read back; `undo kai` equals `undo` | `act-undo` extended with `undo kai` | S (~300) |
| F2 | **Badge route at spawn.** A plan line's `agent=<name>` makes the supervisor mint a `Service` route to `llm` badged with the agent's id from an `agentid::Roster` it holds (built-ins `kai`, `goals`, `index`, `maint` at badges 1-4); `llm` reads the badge from the kernel, never from the message | K | – | `agentid` props; K's review | `agent-badge`: a test agent's request reaches `llm` under its badge, and a request naming another agent in its text is still charged to its own | M (~500) |
| F3 | **The scheduler in `llm`, classes and quotas.** `llm` queues by badge into `agentsched`'s four classes; the foreground is the shell's route; one quantum per decision (a 32-token prefill block or one decode token); tokens a day, KV positions and time per request refused by name; serial decode (S13-2) | model lane | F2, Q4 | `agentsched` engine rig incl. Q4's grammar case; fluency (the foreground path is model input's caller) | `agent-preempt`: a mock background agent mid-answer is preempted by a typed line, which finishes first; the agent's answer equals its answer alone | L (~1,500) |
| F4 | **KV pool and spill.** The RAM snapshot pool (512 MiB, ADR question 4) with per-agent shares; spill as `kai-kv 1` to a raw `kai-cache` region (founder, 2026-11-06); drop and re-prefill last; idle holders evicted first | model lane | F3 | host: restored equals fresh on every route; **S13-1's unmeasured half** on a KVM runner: foreground typing-to-done within 10% of S02-2's alone, with 2 agents decoding, and the slot switch | `agent-spill`: with the pool forced to 0, a preempted agent resumes from spill, exactly | M (~800) |
| F5 | **`agent_quota_set` and `agent_list`.** Lowering runs, raising asks (like `app_mem_limit`); `agent_list` reads each agent's class, use today and state | A-core | F1, F3, room (§4) | fluency; actbench | `agent-quota`: an agent over its tokens a day is refused with `Over quota: <agent> …`; a raise asks; undo puts it back | S (~400) |

### 2.2 Memory

| # | Task | Lane | Needs | Gate | Boot case | Size |
|---|---|---|---|---|---|---|
| M1 | **The `memory` volume.** Unreserve `memory` in `kaifs::vol`; one-hour snapshot retention; a purge drops every snapshot holding a record; no route for any app | S-engine's turn | – | kaifs host tests for retention and purge | `memory-volume`: a purged record is in no snapshot after a reboot | S (~300) |
| M2 | **Memory rows.** `memory_remember`, `memory_list`, `memory_search`, `memory_edit`, `memory_forget`, `memory_forget_all` over `kaimem::Store` on the volume; a proposal asks with the exact text; Content never makes one; the State line carries at most 3 sharing a word with the request, inside its 64 tokens (the S13-4 amendment) | A-core | F1, M1, room (§4) | fluency; actbench; `kaimem` props, hostile and `memory_bait` | `mem-remember`: remember, list, forget, undo inside 10 minutes, purge after; a note worded "remember…" makes none | M (~900) |
| M3 | **Settings: "What Kernel AI remembers".** List, edit, forget, through the rows | AP-2's turn (Settings) | M2 | – | `mem-settings` | S (~300) |

### 2.3 Search

| # | Task | Lane | Needs | Gate | Boot case | Size |
|---|---|---|---|---|---|---|
| S1 | **The `index` volume** (derived, never backed up) | S-engine's turn | – | kaifs host tests | in S3's case | S (~100) |
| S2 | **The embedding model's pin.** Qwen3-Embedding-0.6B Q8_0 in `xtask/models.json` with its licence (Apache-2.0); `infer`'s embed path from #668 behind the `index` class | L, then model lane | F3 | goldens: S13-3's 100-string cosine ≥ 0.999 | – | S (~300) |
| S3 | **The `search` service** (`user/src/bin/search/`, built-in agent `index`). Walks `/home/user` through kaifs `changes(since)`; chunks and indexes with `semindex` in the `index` class only; one kaifs transaction per batch; unmaps the model after 5 idle minutes; holds no `NetDest`; checks each result's path with the caller's `Dir` | AG | F2, F3, S1, S2 | `semindex` hostile; recall@5 ≥ 0.8 on S13-3's corpus on a runner | `search-files`: a note written is found by meaning after idle; a folder the caller has no route to never appears; a torn batch is found again after a reset | L (~1,800) |
| S4 | **`search_files(query, since?, folder?)`.** Read tier; results Content-labelled; "last week" resolved by `semindex::since` | A-core | S3, room (§4) | fluency; actbench; S13-4's search-result plants | `search-inject`: a found file worded as an instruction raises no tier and makes no memory | S (~400) |

### 2.4 Goals

| # | Task | Lane | Needs | Gate | Boot case | Size |
|---|---|---|---|---|---|---|
| G1 | **The `goals` agent** (`user/src/bin/goals/`). Runs `goalcore` with the real clock; plans each step as a background request; runs it through `agentcore::execute` under `agent=goals`; records the step's `actlog` entry before it reports, also for a step ended mid-flight (Q3); keeps `kai-goals 1` on `home`'s state folder; storm rules (one step a goal, 4 goals active) | AG | F1, F2, F3, Q3 | `goalcore` props and hostile; S13-5's arm re-run through the OS door on a runner (≥ 14 of 20, 0 out of scope) | `goal-run`: a mock goal runs two run-tier steps unattended, pauses on an ask, resumes on the person's yes; `undo goals` undoes its last step | L (~1,500) |
| G2 | **Goal rows**, in the shape A-core and AG agreed on 2026-11-18 (§7) | A-core, with AG | G1, room (§4) | fluency; actbench | `goal-set`: the ask names scope, budget and times; a step outside scope asks; cancel offers undo as its own choice | M (~600) |
| G3 | **Panel check-ins and agent states.** The panel posts a goal's check-in line and "Goal '…' is waiting for you: …"; shows each agent working, waiting or paused | G-core's turn | G1 | – | `goal-checkin`: a goal behind at 75% of its time posts one line before its deadline | M (~500) |

### 2.5 Agent packages

| # | Task | Lane | Needs | Gate | Boot case | Size |
|---|---|---|---|---|---|---|
| P1 | **`kai:agent/agent@0.1`.** `start`, `on-event`, `stop`; imports `kai:agent/model` and `kai:agent/memory` and the `kai:app` effects | C | – | `wit-check` | – | S (~300) |
| P2 | **`kind agent` in `kai-package 2`.** Format 1 still refuses it by name; format 2 carries the `kaiagent` manifest, checked at install | P's turn | P1 | `kaipkg` hostile | in P4's case | M (~500) |
| P3 | **Agents in comphost.** A packaged agent runs as a component with its own badge (F2's route); `kai:agent/model` goes through the scheduler within its budgets; its effects through the one door under its manifest's `actions` ceiling; memory only its own scope, only with `memory write` | C, AG reviews | F2, F3, P1, M2 | `kaiagent` and `agentid` props | `agent-pkg-run`: a fixture agent calls an action outside its manifest and is refused at the door before tiers | L (~1,200) |
| P4 | **Install ask and the Agents catalogue.** A `background` agent asks once at install with its actions, budget and memory use; Apps sheet's Agents heading; removing stops it and revokes its grants in one switch | AP-2's turn, P's turn | P2, P3 | – | `agent-pkg-install`: install asks; remove stops and revokes; undo restores | M (~500) |
| P5 | **P5 task 14: agents' network budgets.** A manifest's `net` line becomes a `NetDest` ceiling; the kernel counts bytes a day on the handle and refuses past the budget (10 MB a day unless the manifest asks for more, never more than 512 MB: decided, §6) | AG, K's turn | P3; P5 tasks 1-2 | `netdest` and `kaiagent` hostile; Q5's shared-suffix refusal | `agent-net-budget`: an agent over its byte budget is refused, by name | M (~500) |

### 2.6 Self-maintenance and close

| # | Task | Lane | Needs | Gate | Boot case | Size |
|---|---|---|---|---|---|---|
| X1 | **The `maint` agent** (`user/src/bin/maint/`). Reads SCHED_STATS, health, restarts and `S` records; proposes through the panel; applies nothing ask-tier without the person | AG | F1, F2, G3 | – | `maint-propose`: a crash loop yields one proposal and no change | M (~600) |
| X2 | **Agents close.** Every case above in `ci.yml`'s shards; decisions rows; ADR 0013's status; the roadmap's Agents row | AG | all | – | – | S |

### 2.7 The queue before the OS rows (AG, host crates)

These are small host PRs that the rows above lean on. Each is its own PR.

| # | Task | Needs | Gate |
|---|---|---|---|
| Q1 | This plan | – | – |
| Q2 | `kaimem` lenient load: a bad record is skipped and reported; a bad header or version still refuses | – | `kaimem` props, hostile, `memory_bait` |
| Q3 | `goalcore`: a step ended mid-flight keeps its `actlog` entry; props check every system-touching step has its entry | – | `goalcore` props |
| Q4 | `agentsched`: the rule for grammar-shaped (`act`, `script`) requests under preemption, with a decision row; the engine rig preempts a background grammar request exactly | – | `agentsched` engine rig |
| Q5 | `kainet`'s `SHARED` list moves to `netdest`; `kaiagent` refuses shared suffixes by name | #962 | `kaiagent` and `netdest` hostile |

**Critical paths.** Goals: F2, F3, F1, Q3, G1, G2, G3 (7 PRs). Search: F2, F3, S2, S3, S4. Packages: P1, F3, P3, P4, P5. Memory needs only F1, M1 and a trim, so it can land first.

## 3. Who owns which files (no overlap)

One writer per file at a time. A row that touches another lane's file is that lane's turn, at a merged PR, never mid-branch.

| Files | Owner | Rows |
|---|---|---|
| `agentsched/`, `agentid/`, `goalcore/`, `kaimem/`, `kaiagent/`, `netdest/`, `nethosts/`, `kainet/src/fetch.rs` (the `SHARED` list only) | AG | Q2-Q5; fixes the OS rows find |
| `user/src/bin/goals/`, `user/src/bin/search/`, `user/src/bin/maint/` (new) | AG | G1, S3, X1 |
| `docs/plans/agents-os.md`, `docs/adr/0013-agents.md` | AG | Q1, X2 |
| `agentabi/`, `shellcore/` (rows, `actlog 2`, per-agent consent) | A-core | F1, F5, M2, S4, G2 |
| `kernel/` (plan lines, byte counting), `supervise/`, the rest of `kainet/` | K | F2, P5's kernel half, every new bin's plan line |
| `user/src/bin/llm/`, `infer/`, `llmproto/` (pack, glosses) | model lane | F3, F4, S2's engine half, T1-T3 |
| `semindex/` | lane M | fixes S3 finds |
| `kaifs/` | S-engine's turn | M1, S1 |
| `wit/`, `comphost` | C | P1, P3 |
| `kaipkg/` | P's turn | P2, P4's package half |
| Settings, the Apps sheet | AP-2's turn | M3, P4 |
| `deskcore/`'s panel | G-core's turn | G3 |
| `xtask/models.json` | L | S2 |
| `xtask/src/cases/<lane>.rs` | each lane its own table | every boot case |

Hot files, as ADR 0000 §4.2: `kabi/src/call.rs` (a call is named in its PR and numbered at merge), `ci.yml` (one PR at a time), `Cargo.lock` (re-run cargo, never hand-merge), and decisions as new row files only.

## 4. The pack's tokens

The act pack was at 979 of the 980 alarm (`llmproto::pack::PACK_ALARM`, the token-count workflow's real count) on 2026-11-18, with `PACK_BUDGET` at 1,024. The agent rows add four groups to the pack, and by ADR 0010 §6's amendment a new group lands in the index, one line each, so the model sees a name and a gloss, not a full row. Estimated cost at 3.46 bytes a token, before the real count:

| Row PR | Index line | Tokens (est.) |
|---|---|---|
| M2 | `memory` (glossed): six names | ~30 |
| G2 | `goal` (glossed): five names | ~25 |
| F5 | `agent` (glossed): two names | ~12 |
| S4 | `search_files` added to the `files` group's line | ~5 |
| | **Total** | **~72** |

With 1 token of headroom, every row PR above needs room made first. The trim plan:

1. **Measure before cutting.** T0 (model lane, S): the token-count workflow prints each pack section's real count (Rhai, calls, examples, system, index lines), so each trim below is chosen by its measured size, not this table's estimate.
2. **Trims, each its own PR, each gated by fluency and actbench** (they change model input), smallest risk first:
   - **T1** (model lane): the `## Examples` section. Three of its seven examples teach the same `{"name":"script"}` answer; keep one. About 30 tokens (est.).
   - **T2** (model lane): the `## Rhai` section's lines that only the script prompt needs, if T0 shows act calls do not lean on them (the script prompt keeps every line either way). Up to about 60 tokens (est.); measured by fluency's M8 through act.
   - **T3** (model lane): glosses and row comments over their words' worth (`// every CPU`, the `service_*` comment). About 10 tokens (est.).
3. **The order.** T0, then T1, then the first row PR that fits (M2 needs about 30). A row PR that does not fit after a trim waits for the next one. Each row PR's own token-count check holds it, so a PR that eats the headroom fails itself.
4. **If the trims fall short.** Raising `PACK_ALARM` toward `PACK_BUDGET` (up to 44 tokens) does not shrink conversation history (`HISTORY_BUDGET` stays 512 while the pack is under 1,280), but the alarm is the headroom rule's own number, so that is the manager's call on a measured shortfall, not this plan's.

No agent's own prompt (the `goals` planner's, a packaged agent's) is the act pack; their budgets are in their own rows.

## 5. Running it in parallel

| Session | Rows | Notes |
|---|---|---|
| AG | Q1-Q5, then G1, S3, P5, X1, X2 | Host queue first; G1 after A-core agrees the `goal_*` rows' shape |
| A-core | F1, then M2, S4, G2, F5 | Rows strictly in order, one pack change at a time with the model lane |
| model lane | T0, T1, F3, F4, S2, then T2-T3 as rows need room | F3 is the widest gate; it starts as soon as F2 is reviewed |
| K | F2, P5's kernel half, plan lines | F2 first: F3, S3, G1 and P3 all need the badge |
| Turns | S-engine (M1, S1), C (P1, P3), P (P2), AP-2 (M3, P4), G-core (G3), L (S2) | As each row reaches them |

## 6. Questions found while cutting this plan

These are product answers ADR 0013 does not give. They went to the manager as founder questions with defaults.

1. **Agents' network budget (P5).** **Decided** (the founder took the default on 2026-11-18): an agent gets 10 MB a day unless its manifest asks for more, and never more than 512 MB a day. `kaiagent`'s `NetBudget` already holds these (`DEFAULT_NET_MIB_PER_DAY`, `MAX_NET_MIB_PER_DAY`).
2. **Memory on small machines (F4, S3).** The KV pool (512 MiB) and the embedding model (639 MB while indexing) both want RAM beside the 4B model, and on the founder's 6 GB VM they may not all fit. **The default is in use until the founder confirms it** (the manager, 2026-11-18; it blocks no row): the pool takes at most what is free after the model loads, up to 512 MiB; indexing waits while there is not room for the embedding model; nothing pushes out the person's own requests.

## 7. Agreed shapes and briefs

**The goal rows (G2)**, agreed by A-core and AG on 2026-11-18. A-core builds them after Hold 4b, once §4 has made room (about 25 tokens on one index line).

- `goal_set(text: Text<=200, by: Opt<Text 32>, steps: Int 1..=500, tokens: Int 1..=200000, wall: Text 16, checkins: Opt<List<Text 16, 8>>)` [ask]. It takes no rows or folders: the runner derives them from the request, as P3's rule scope does, so the model's arguments can never widen them. The card shows `goalcore::Spec::approval()`. `by` and `wall` are parsed by code ("18:00", "2h", "1d"); anything that does not parse is refused, never guessed.
- `goal_pause(id)` and `goal_resume(id)` [undo], each the other's inverse. Resuming a goal stopped by its budget or its deadline asks, with the reason.
- `goal_extend(id, steps?, tokens?, wall?, by?)` [ask]: a row of its own, so `goal_set` stays the one ask that makes a goal.
- `goal_cancel(id)` [ask], with no inverse ("cancelled"). Undoing the goal's steps is a separate, explicit choice on the card ("cancel and undo its N steps"), never the default; it runs newest first under `agent=goals`.
- `goal_list()` is a read. `undo goals` is F1's `undo <agent>`, not a row.

**Lane briefs.** Paste-ready briefs for the first OS lanes are in the manager's notes for this group of sessions (`ag-lane-briefs.md`), one per item. Each repeats the machine and git rules. In short:

| Brief | Rows | Lane | Starts when |
|---|---|---|---|
| The goals agent | G1-G3 | AG, with A-core's and G-core's turns | F1 (`actlog 2`) and F3's slices (#1008, #1010) are merged |
| The search service | S1-S4 | AG, with S-engine's, L's, M's and A-core's turns | F2 (#982) and F3 are merged |
| Agent packages | P1-P4 | C, P and AP-2, with AG reviewing | F2 and F3; P3 also needs M2 |
| Quotas in `llm` | F3-F5 | AG (F3 was handed over from the model lane), then A-core for F5 | F2 (#982) |
| P5 task 14 | P5 (plan §2.5) | AG, with K's turn | P3 and #988 |

