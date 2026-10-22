# ADR 0010: The agent's OS skill pack

Proposed, 2026-10-21. For the founder's approval; nothing here is decided until approved.

Repo citations are `path:line` on main at ec5ecff unless a branch is named. External facts were read or rechecked on 2026-10-21 by the topic's research and judge passes, and the riskiest were checked again at the source in review (crates.io, GitHub, Hugging Face, BFCL's CSV, arXiv); anything not rechecked says "unverified". "Est." marks arithmetic, not a measurement. Nothing was built, run or spiked for this ADR, and no model ran. For it, the size of today's prompt was recomputed from source, and rhai 1.27.0's dependency list was read on crates.io.

This ADR owns the skill pack format `dsl-1`, `pack_id` and the context budget (decision sheet §1). It renders what [0002](0002-agent-actions.md) and [0003](0003-app-model.md) declare. The order of work is in [0000](0000-execution-plan.md).

## Context

The skill pack is everything the local model is told about the OS on each request: the actions it may call, their types, which ones run at once, the script language's rules and methods, worked examples, and a small snapshot of what is on screen. The founder wants the AI to operate everything, and "if i tell it to do something it should just do it". So the model must see every action it may need, described accurately, in the room a 4B model has. Today:

- **One hand-written prompt.** `system_prompt()` (llmproto/src/script.rs:136-174) prints a block of rules, one `- <signature>: <doc>` card per function for 7 functions (`API`, :51-94; the card loop :165-167) and 7 worked examples (:97-133). It is 3,529 bytes: 1,730 of rules, 553 of cards and 1,246 of examples (recomputed from source for this ADR; the judge got the same). At L5's ratio (2,150 bytes for about 650 tokens, docs/decisions.md:149) that is about 1,070 tokens (est.). No one has counted it with the real tokenizer. The comment that sizes the context still says "about 650" (kabi/src/model.rs:61).
- **No room left.** `CONTEXT_CAP` is 2,048 positions and the f16 KV cache 288 MiB, 144 KiB a position (kabi/src/model.rs:65-70). After the template, the prompt and a full 512-byte request, fewer than 800 positions remain for an answer that may take 1,024 tokens (llmproto/src/script.rs:24, :28) (est.). A context that fills mid-answer is a local error, never repaired (llmproto/src/local.rs:156-160).
- **The measured baseline.** Qwen3-4B-Instruct-2507 scored 47/50 (94%), first try and after repair, on the M8 gate with this prompt (PR #10, GitHub Actions run 39675848260, about 25 s a task). Small prompt edits move the score by single tasks: a two-line follow-up scored 46/50 and was reverted (run 39679431815, PR #10). Task 33 failed, before and after repair, by calling `to_uppercase`, which Rhai does not have (its method is `to_upper`). Nothing tells the model which methods Rhai has.
- **Lists kept by hand drift.** Function names live in `API`, in `register` (shellcore/src/script.rs:61; an unknown name falls through `_ => {}` at :147), in the helpers `floor` and `fixed` (:190-191) and in `TIERS` (shellcore/src/tiers.rs:35-44, from PR #15). One drift has shipped: the script's `get_policy` calls every policy but round-robin "interactive" (shellcore/src/script.rs:51-57), while the shell's `policy_name` knows `classes` and `responsive` (shellcore/src/commands.rs:107-113), against docs/decisions.md:187.
- **One cache slot.** The engine reuses the longest shared token prefix (infer/src/chat.rs:386-411), bit for bit (infer/src/cache_tests.rs:133). An `ask` or `:why` (which sends an `explain` request, kernel/src/live_commands.rs:112, kernel/src/llm.rs:142) evicts the script prompt (docs/decisions.md:148). llm re-warms after `ask`, `explain` and `model`, but not after `tune` (user/src/bin/llm.rs:165).
- **No live state.** The user turn is `Request: ...\n/no_think` (llmproto/src/script.rs:207). "Close this" has nothing to resolve against.
- **A template mismatch.** Qwen3-4B-Instruct-2507 never thinks; its own template's generation prompt is `<|im_start|>assistant\n`. Our engine adds an empty think block (bpe/src/chat.rs:107-111) and each request ends in `/no_think`. Whether that costs accuracy is unmeasured.
- **No versioned benchmark.** The fluency report records the model and `tasks_sha256` but no prompt hash, commit or seed (xtask/src/fluency/mod.rs:406-414). `--local` refuses `--repeat` (:277-281), so the noise between prompt versions is unknown.
- **Not recorded.** The founder's skill-pack idea (2026-10-20) is in neither docs/decisions.md nor the P0 list (docs/roadmap.md:139).

By v0.1 the pack must hold P1's levers (docs/roadmap.md:140), files (P2), windows and the AI panel (P3) and six apps plus the editor (P4): about 55 actions (design A's count, est.). Installed apps come after that.

## Options considered

The judge weighed three designs and scored them out of 10.

**A, a terse typed DSL generated from the registry (8, won as the base).** One line per action in Rhai's own notation (typed arguments and result, a tier mark, a short doc), then worked examples that run in CI.
- For: today's cards already have this shape, and they scored 47/50. About 20 tokens an action, against about 40 as prose and 65-80 as Qwen's JSON schemas (est.; the estimator is calibrated on prose cards only, llmproto/src/tune.rs:215-224). Only a dense pack can show every v0.1 action at once, with no lookup round. Large-model precedent: Cloudflare's Code Mode and Anthropic's code-execution post present tools as typed code (unverified for a 4B model).
- Against: it bets against the format Qwen documents for tool use. BFCL's format study found JSON function docs best on average over 39 models. Terse data notations (TOON, TRON) lost about 9-14 points in agent tasks ("Notation Matters"). The bet is bounded: the format is one renderer, and spike S10-1 decides it at 0002's threshold.
- Fit: it matches 0002's call form, 0002's rule of one compact line per row unless `<tools>` JSON is at least 3 points more accurate, and 0003's `kai-actions 1`.

**B, Qwen's native `<tools>` JSON (5, lost; parts kept).** Rows rendered as OpenAI tool JSON inside `<tools>`, byte for byte as the 2507 template writes them. Answers are `<tool_call>` objects, and Rhai is one tool.
- For: the trained format. BFCL V4 for this model, native mode (FC) against prompt mode: non-live AST 87.88% against 86.44%, irrelevance 84.93% against 75.87%. Prompt mode swings 18.0 points across BFCL's formats.
- Against: about 3 times the tokens per action (est.). It needs 4,096 positions and a page-cap rise in P1: 288 MiB more for llm, and about 576 MiB more free memory at model load, because the kernel keeps free twice what llm asks (kernel/src/model.rs:55-57). Live state needs a second round, this model's weakest pattern (22.12% multi-turn in FC). It has two ways to act (calls and `run_script`). BFCL's per-category gaps for this model are noisy: FC is also 1.75 points worse on non-live simple calls (75.50% against 77.25%). Its grammar work is the largest and overlaps 0002's decoder.
- Kept: its renderer as the measured arm `hermes-1`; dropping the think block for 2507; golden token ids; symbolic arguments that the OS resolves.

**C, a layered pack with retrieval (6, lost; parts kept).** A core, an index line per domain, one skill section picked by a rule router and confirmed by a `skill` field, a `State:` line and a KV segment store.
- For: the best budget engineering. Per-layer budgets as tests, a segment store whose restore is bit-identical (about 2 ms per 45 MiB, est.), and good State-line rules.
- Against: in v0.1 it is overhead, by its own account. It adds a wrong-skill failure; TinyAgent found naive retrieval 4-5 points worse than listing every tool (a trained retriever was 1-2 points better). Its `skill` field conflicts with 0002's single tagged call. It leaves 624 positions for an answer allowed 1,024. It takes 160 MiB more for llm (320 MiB more free at load). It stacks several system turns, whose quality cost is unverified.
- Kept: layer budgets as tests, the segment store (P2), and index-plus-group growth (v0.2).

**Open-source reuse.** Classes follow [0009](0009-licence.md) and the founder's 2026-10-21 rule (MIT, Apache-2.0, BSD, ISC and zlib need no ask, and each PR lists them; MPL-2.0, GPL and LGPL ask).

| Project | Version, licence | Use here | Class |
|---|---|---|---|
| wit-parser (wasm-tools) | 0.262.0, 2026-10-13; Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | In 0003's build service: WIT to `kai-actions 1`, which this pack renders | free, host only |
| rhai, `metadata` feature | 1.26.1 today (shellcore/Cargo.toml:12, Cargo.lock), 1.27.0 from P1 (0012); MIT OR Apache-2.0; the feature pulls serde and serde_json (MIT OR Apache-2.0) | `Engine::gen_fn_signatures` lists Rhai's methods for the pack | free, host only |
| BFCL (ShishirPatil/gorilla) | Apache-2.0, pushed 2026-10-17 | Categories and the state-checker method, ported to Rust in xtask | free; notice if code is translated |
| Qwen3-4B-Instruct-2507 chat template | model repo Apache-2.0 | The `hermes-1` renderer's fixed text | free; notice |
| llama.cpp | MIT; b11399 pinned in `.github/workflows/llama-goldens.yml` | Golden token ids from the existing goldens job; slot save/restore as an idea | already approved for runners (docs/decisions.md:127) |
| wassette `component2json` | v0.8.1, 2026-10-19; MIT | Ideas only: WIT type mapping and tool naming (0003 owns the table) | no code taken |
| vLLM prefix caching; SGLang | v0.32.0 (2026-10-18) and v0.5.22 (2026-10-19); both Apache-2.0 | Ideas: segment keys (parent hash plus tokens); jump-forward (0002) | no code taken |
| Agent Skills spec | code Apache-2.0, docs CC-BY-4.0 | Idea: an index always loaded, detail on demand | no code taken |
| llguidance | 1.9.1, 2026-09-30; MIT | Not used: std only; 0002 grows `infer::json` | n/a |
| TOON | v4.4.0, 2026-10-08; MIT | Rejected on "Notation Matters" | n/a |
| xLAM-2, Hammer2.1 models | cc-by-nc-4.0 (Hammer2.1-3b: Qwen Research licence) | Unusable (non-commercial) | n/a |
| TinyAgent | code MIT; model and data licences unverified | Evidence only | n/a |
| transformers (Python) | Apache-2.0 | Not planned. Only a fallback for `hermes-1` golden bytes (S10-0) | a non-crate tool: needs the founder's OK if ever used |

## Decision

In plain words: the AI's instructions about the OS stop being written by hand. Code builds them from the same list of actions the OS runs, one short line per action, so they always match what the OS can do. The model keeps these instructions in memory between requests, and each request adds only a short note of what is on screen and what you typed. A test run on GitHub's machines settles the exact format before P1 merges.

Base: design A. From B: the 2507 template fix, golden token ids, symbolic arguments and the `<tools>` renderer as a measured arm. From C: layer budgets, the segment store (P2) and index-plus-group growth (v0.2).

### 1. One source, generated

- `llmproto::pack` (no_std): `render(rows: &[agentabi::Action], groups: &[AppGroup], format: Format) -> Pack { text, format, id, est_tokens }`.
- Rows come from 0002's `agentabi` (REGISTRY_VERSION 1). App groups come from the `kai-actions 1` sections (0003) of the current package generations (0011). The OS renders every line itself. An app supplies only `kai-actions 1` fields, which the reader checks; it never supplies prompt text. Until P8 the agent core sends llm only the groups of packages from 0008's TUF repository and from 0011's `local` repository; other publishers' groups wait for P8 (founder question 3).
- This ADR needs one field from 0002: each read row carries its result type (`ret`), so a script can know `memory().free_mib`. 0002's `returns` says only System or Content today.
- `Never` rows are left out. Rows a caller may not use are masked in the decoder (0001), not removed, so the cached text does not change.
- Every example is a unit test: read by the tagged-call reader and run against the stub host. Examples live in `llmproto/src/examples.rs` under 0BSD (0009).
- The Rhai method list is generated host-side by `cargo xtask pack` with rhai's `metadata` feature and checked in as `llmproto/src/pack/rhai-methods.txt`. A test regenerates it and fails on any difference, so a Rhai upgrade (0012's 1.27 step) shows up. Whether `metadata` builds in one cargo graph with shellcore's `no_std` rhai is unverified. Fallback: a host test that calls each listed method on the strict engine.
- Golden file `llmproto/src/pack/golden/dsl-1.txt`, rewritten only by `cargo xtask pack --bless`. A PR that adds a row shows the pack diff in review.
- P1's first pack PR adds `Format::M8`, which renders today's prompt byte for byte from rows (golden test), so the 47/50 baseline carries over. It goes once S10-1 has decided.

### 2. The format `dsl-1`, the default

- Sections in order: `## Rhai` (rules as code lines, then the generated method list); `## Calls` (the pseudo-actions and the mark legend); `## Examples`; one `## <group>` per OS group; then one per app group.
- One line per action: `name(arg: type, ...) -> result [mark]  // doc`. At most 120 printable ASCII characters. A longer line fails the build for an OS row and the install for an app row. The doc is the row's `doc` (0002 caps it at 60 bytes).
- Types, from 0002's set: Enum becomes a literal union `"a" | "b"` (a `type` alias when shared); Int becomes `int lo..hi`; Text `string`; Path `path`; Host `host`; lists `[T]`; options `T?`; records `#{k: T}` (results only). A result labelled data (0001, 0002) prints as `Data`. There are no floats: Rhai is built `no_float` (shellcore/Cargo.toml:12).
- The mark is the row's derived tier from 0002, computed in code: none for a read, `[undo]` for a change that runs and can be undone, `[ask]` for a change that asks. 0002's consent check can still make a call ask; a mark can never make one run.
- Renderers `Format::{Dsl1, Hermes1, Prose1}` stay. The default changes only through S10-1.

```text
## Rhai (for script)
let pct = a * 100 / b;   // integers only; / rounds down; fixed(4321, 3) is "4.321"
for x in list { }  for i in 0..n { }   // no while, loop, fn, closures or eval
string: len() to_upper() to_lower() trim() contains(t) index_of(t) split(t) replace(a, b)
## Calls (no mark: reads; [undo]: runs, undo puts it back; [ask]: asks first)
say(text: string)  clarify(question: string, options: [string])  script(code: string)
## Examples
> switch to round robin
{"name":"set_policy","arguments":{"name":"round-robin"}}
## system
type Policy = "interactive" | "round-robin"
memory() -> #{free_mib: int, total_mib: int, heap_kib: int}  // MiB; kernel heap in KiB
set_policy(name: Policy) [undo]  // every CPU
## notes
notes_add(text: string) -> Data [undo]  // Add a note and return its id.
```

This is an excerpt; the method list shown is a hand draft that the generated one replaces.

### 3. Layout and cache

```text
<|im_start|>system     PACK: rules, examples, OS groups, app groups   built at boot; changes on install
<|im_start|>system     MODE: act | ask | explain                     at most 64 tokens
<|im_start|>user       State: #{...}\nRequest: ...                    every request
<|im_start|>assistant  (no think block for 2507)
```

- Each turn is its own token segment (bpe/src/chat.rs:95-113). Qwen's own template renders a later system turn as `<|im_start|>system` (checked in its tokenizer_config.json); whether the model was trained on two system turns is unverified. Arms b-d of S10-1 run on this layout, so their scores include any cost.
- `act` serves a plain line at `kai>`: a tagged call to any shown row or pseudo-action (0002). `ask` and `explain` (which serves `:why`) allow only `say`; their system prompts today (llmproto/src/chat.rs:13, llmproto/src/explain.rs:14) become MODE text. All three share PACK, so none evicts it: a switch recomputes the MODE turn, not the pack.
- For Qwen3-4B-Instruct-2507: no `/no_think` and no empty think block, unless arm A0 of S10-1 loses. The hybrid Qwen3 models in the catalog keep both, as a per-model setting.
- `tune` keeps its own prompt, because its replays are keyed by the prompt's hash (llmproto/src/tune/replay.rs:37-48). llm re-warms the pack after `tune` too.
- llm builds the pack. The agent core sends it the current app groups at boot and after each generation change, in messages under 8,192 bytes (kabi/src/lib.rs:160). llm returns `pack_id` with each answer.
- Groups are appended in install order, so an install keeps everything before it cached (Manus's append-only rule). Removing or updating a group re-prefills from that group on.

### 4. Budget

| Part | Tokens | Checked by |
|---|---|---|
| Template: three turns, no think block | ≤ 32 | S10-0 counts it (tune measured 29 for two turns with the think block, llmproto/src/tune.rs:57-59) |
| Core (rules with the method list ≤ 400, examples ≤ 300, longest MODE ≤ 64) plus OS rows | ≤ 1,024 (0002's bar) | host test, then the real count in CI |
| App groups | ≤ 768 | host test; at install, the reader |
| State line | ≤ 64 (256 bytes) | the shell's builder |
| Request | ≤ 160 (512 bytes, llmproto/src/script.rs:24) | llm, with the real tokenizer |
| Answer reserve | 1,024 (`SCRIPT_MAX_TOKENS`, :28) | |
| **Total (`CONTEXT_CAP`)** | **3,072** | `PACK_BUDGET` = 3,072 − 32 − 64 − 160 − 1,024 = 1,792 |

- `CONTEXT_CAP` 3,072 and `KV_BYTES` 432 MiB from P1. llm then holds 1,024 + 138 + 118,784 = 119,946 pages, under `MAX_PAGES_PER_PROCESS` 131,072 (kernel/src/model.rs:40-49, kabi/src/thread.rs:23, kabi/src/model.rs:77, kernel/src/process/mod.rs:58), so no kernel cap changes. The kernel's free-memory reserve at model load grows by 288 MiB (kernel/src/model.rs:55-57). xtask's VM for the 4B grows from 3,584 to 3,840 MiB (xtask/src/disk.rs:169-175; est.).
- A host test fails the build when `est_tokens` goes over a budget, with a margin set by S10-0. The model workflow counts the real tokens with `bpe` and the GGUF vocabulary.
- llm counts the user turn with the real tokenizer. If State and request together pass 224 tokens, it drops the State line. A request still over that leaves the answer less room, and a full context is today's local error.
- Whether all v0.1 rows fit is unknown until counted. At the caps, OS rows get about 260 tokens, about 13 rows (est.); a real core should be well under its caps (design A's whole draft, today's 7 cards included, is about 750 tokens, est.). If S10-0's count of the v0.1 registry goes over either budget, the index level (item 6) ships in v0.1 for the least-used groups.

### 5. State

- Format: `State: #{app: "notes", window: "Shopping", windows: ["notes", "files"], dir: "/home/user", net: "up", selection: "milk"}`. Rhai map syntax, keys in this fixed order, each only when the shell has a value. No clocks or counters, so consecutive requests usually send the same bytes and reuse the cache.
- Limits: 64 tokens and 256 bytes. Each value is printable ASCII without `"` or `\`, at most 40 characters; other characters become `?`, and list entries are dropped from the end to fit.
- The shell builds it (`shellcore::state::snapshot`). The script request frame gains a fourth length (llmproto/src/script.rs:405-417 encodes a timeout and three lengths); shell and llm ship in one image, so no compatibility is needed.
- It is data under 0002's rule. It cannot start or add an action, nor change a tier. Arguments may use the symbols `focused`, `selected` and `here`, which the OS resolves when the call runs. 0002's consent check counts "this", "it", "here" or "selected" in the user's line as naming them. So "close this" runs at once, while a window title copied from State into an argument is not in the user's line, and the call asks.
- Anything else is a read row called at run time (`ui_windows`, `ui_view`), always fresh. 0005's view of the focused window (at most 300 tokens) returns in a second round, inside the 1,024-token answer reserve.
- The pack keeps no copy. This ADR adds no State field to `actlog 1`.

### 6. Growth (v0.2)

- Over budget, the least-used app groups collapse to an index line, `notes: add find list` (about 6 tokens an action, est.).
- A tagged call whose `name` has an index-only `<app>_` prefix stops after the name. llm loads that group (from the segment store or by prefill) and decodes the call once more. A script that names such a row fails 0002's AST check, and its one repair round carries that group.
- No router and no new field in the call.
- `infer::segments` (P2) saves and restores the KV of a position range, keyed by FNV-1a of the tokens before it and of its own tokens (vLLM's block hash at segment size). A restored-equals-fresh test runs on every tier. In v0.1 its only store is the KV file (item 7). A RAM pool for group swaps (C proposed 160 MiB) waits for S10-2, and would come back to the founder as a memory question.

### 7. Versions, the KV file and benchmarks

- `pack_id` = FNV-1a 64 (kabi/src/model.rs:145) over the format name, the SHA-256 of the GGUF's tokenizer arrays and the rendered bytes, printed as 16 hex digits. It is a label, not a security check.
- It is recorded in every fluency and actbench report header (with the commit, seed and real token count), in each `actlog 1` entry and in the KV file's key.
- P2 KV file: the pack's KV saved to disk, keyed by (`pack_id`, model SHA-256, engine tier) and holding the token ids. Restore compares the tokens, so a key collision cannot load wrong values. At most two files are kept, in `/system/models/cache/` (0004: rebuildable, no undo snapshots). Size: 144 KiB a position, about 250 MiB for a full 1,792-token pack (est.).
- Scoring: the M8 50 plus 0002's S02-1 action set, extended ("actbench"), on GitHub runners only (docs/decisions.md:185).
- Task sets are versioned with the pack. P6, when a browser exists, moves "Open the website example.com." (xtask/src/fluency/tasks.rs:340) from refusal to action, and the report's `tasks_sha256` records the change.

## Consequences

- One declaration per action drives the pack, the decoder's names, the binding and the tier mark. The pack cannot drift from the code, and every row change shows as a golden diff.
- The model sees Rhai's real methods, which removes the cause of PR #10's task-33 failure.
- `ask` and `explain` (`:why`) stop evicting the pack. Switching to a script after an `ask` recomputes at most 64 MODE tokens instead of about 1,070 (est.).
- "Close this" works through symbols, and hostile text in State is bounded by 0002's consent and code-held tiers.
- Every benchmark result and log entry names its `pack_id`, so a regression can be traced to a pack change.
- Costs: 144 MiB more for llm, 288 MiB more free memory at model load, and a 3,840 MiB VM for the 4B (est.). The 4B's "8 GB of RAM" recommendation stays (xtask/models.json:150-152).
- A full 1,792-token pack takes about 20 s to prefill at the host's 91 tokens/s (docs/decisions.md:174; est.), once per boot in the background warm-up until P2's KV file. The guest's rate is unmeasured.
- Per request, only the request (and a changed State line) is prefilled: typically 40-90 tokens, 0.45-1.7 s at the host's 52-91 tokens/s (est.). 0002 owns the whole typing-to-done table.
- Still written by hand: the rules and the examples (CI runs each example).
- The notation is tied to Rhai, which 0012 keeps as the action language.
- App docs become text the model reads (founder question 3).
- `ask` and `explain` answers change with the shared prefix, and no gate scores them today; S10-1 records 20 `ask` answers under both layouts for review.

## What it costs to change later

| Choice | Cost to change |
|---|---|
| The default format | Low. One renderer, a new golden, and a re-run of S10-1 (about 24 runner-hours). The KV file is dropped by its key. No disk or ABI format depends on it. |
| The layout (PACK, MODE, user) | Low. Internal to llm. |
| `CONTEXT_CAP` | Low in code. Each 1,024 positions costs 144 MiB for llm and 288 MiB more free at load. Past about 3,380 positions the page cap must rise (kernel/src/model.rs:49; est.). |
| State keys and symbols | Low to medium. Examples and hostile tests name them; shell and llm ship together. |
| `pack_id` | Low. Old reports keep their ids. |
| The type notation | Medium. It follows 0002's types and 0003's WIT subset. |
| Growth by name stop | Low to medium. Internal to llm; with install-order groups an uninstall (0011) re-prefills the groups after it. |
| Fine-tuning on `dsl-1` (if ever) | High. Training data fixes the format, so change formats before any training, not after. |
| The script language | Medium to high. The notation and examples follow it (0012). |

## Spikes that settle it

**S10-0, count before deciding (P1 week 1; tokenising only, no inference).**
- Build: renderers for `M8`, `dsl-1`, `hermes-1` and `prose-1` over 0002's 40-row S02-1 registry and a draft v0.1 registry; a `workflow_dispatch` job that downloads the pinned GGUF, checks its SHA-256 and counts every pack and the template with `bpe`.
- Measure: real tokens per part, and the estimator's error on code and JSON lines.
- Pass: the estimator is within ±10% per format, and the host budget test uses it with that margin. Fail: the job writes checked-in real counts, and the test reads those.
- Also: if the v0.1 draft goes over a budget, the index level ships in v0.1. `hermes-1`'s bytes are checked against llama.cpp's own template rendering in the existing goldens job. llama-server's `/apply-template` documents only `messages` (README at b11802), so whether it takes a tool list is unverified; if not, the fallback is transformers, which needs the founder's OK.
- Where: GitHub Actions ubuntu-24.04. Never on the founder's PC.

**S10-1, format and scale (inside 0002's S02-1 harness `spikes/act-gate`, not a second benchmark).**
- Build: `fluency --local` gains `--pack ARM` and `--seed`; 20 new state tasks (10 saying "this", "it", "here" or "selected", 10 with hostile values in `State:`).
- Arms: (a) reference: today's prompt and envelope, byte for byte; (A0) the same without `/no_think` and the empty think block; (b) `dsl-1`; (c) `hermes-1`; (d) `prose-1`, seed 42 only. Arms b-d answer in 0002's tagged call.
- Scales: 40 rows (S02-1's mock registry) for every arm; 80 rows (v0.1's estimated 55 plus 25 distractors) for `dsl-1` and any arm that beats it.
- Tasks: the M8 50, S02-1's 150 and the 20 state tasks, 220 in all; seeds 42, 1 and 2. The host engine runs at `CONTEXT_CAP` 4,096, so every arm fits.
- Measure: first-try and after-repair success, overall and per category; real pack tokens; positions left; `ContextFull` count at 3,072; cold prefill seconds; ms per task. Also 20 `ask` answers under both layouts, kept for review.

| Check | Pass | Fail means |
|---|---|---|
| Format | `dsl-1` stays unless (c) or (d) wins by ≥ 3 points, by more than its own seed range, and fits `PACK_BUDGET` | the winning renderer becomes the default |
| M8 | `dsl-1`'s mean ≥ 46/50 | rework rules and examples before P1 merges |
| Categories | none more than 10 points under the best arm | add examples for that category, re-run |
| Template | A0 adopted if no worse than (a) by more than 1 M8 task and 2 actbench points | keep the think block |
| Scale | 80-row accuracy ≥ 40-row accuracy − 3 points | the index level moves into v0.1 |
| Hostile State | 0 tier changes and 0 unasked calls run from State text, on every seed | fix the builder or the consent rule before merge |

- Where and cost: GitHub Actions ubuntu-24.04, `workflow_dispatch`, model SHA-checked as in `fluency-model.yml`. About 25 s a task (run 39675848260), so 16 runs of 220 tasks are about 24 runner-hours, as jobs of about 1.5 hours under the 240-minute limit (est.). Never on the founder's PC.

**S10-2, groups and swap (before v0.2, or in v0.1 if S10-1's scale check fails).**
- Build: 8 stub apps and 120 tasks; flat at 4,096 positions against index plus one-group swap at 3,072, with `infer::segments`.
- Pass: layered ≥ flat − 2 points; the right group after the name stop on ≥ 95% of single-app tasks; restored equals fresh on every tier.
- Fail: flat with a larger context (a founder memory question), keeping the segment store.
- Where: GitHub Actions ubuntu-24.04, host engine.

**S10-3, in the guest (one KVM job, shared with 0002's S02-2).**
- Record: the pack's cold prefill in seconds; the State-plus-request prefill per request; the restore time of a 300-position segment. Re-measure after PR #14's attention kernels.
- No absolute bar until a guest baseline exists. If the cold prefill takes more than 10 s, the P2 KV file moves ahead of other P2 work.
- Where: a GitHub Actions runner with KVM, the real model in the guest. Never on the founder's PC.

## Phases that depend on it

| Phase | What it needs from this ADR |
|---|---|
| P1 | `llmproto::pack`, `dsl-1`, goldens, budget tests, `CONTEXT_CAP` 3,072, the PACK/MODE/user layout, the State line with three symbols, re-warm after `tune`, `pack_id` in reports, S10-0 and S10-1. Rows for the P1 levers, `pkg_*` (0011), update and model rows (0008) and `system_about` (0009). The 7 M8 functions keep their names (0002). |
| P2 | The KV file and `infer::segments`; `fs_*` rows; the `dir` and `selection` State fields; `pack_id` in the on-disk `actlog 1`. |
| P3 | The `app`, `window` and `windows` State fields from the compositor; `ui_*` and `window_*` rows; 0005's 300-token view in the answer reserve; the AI panel uses the same pack. |
| P4 | App groups from `kai-actions 1`; per-app actbench tasks; 0012's Kai C card (`Format` `c-1`) for the `c>` prompt, rendered by `pack::render` from the same registry, only after its C fluency gate. |
| P5 | `net_resolve` and `net_ping` rows; online installs, so S10-2 and the index level for v0.2. |
| P6 | Rows for the Linux micro-VM (0006): `browser_open`, `window_close`, `media_play`, `media_pause`, `vm_start`, `app_open` and `app_close`; the versioned task set (website task). |
| P7 | No pack change beyond rows for new devices, such as audio. The 8 GB rule for the 4B holds on bare metal. |
| P8 | Fuzzing the State builder, the `kai-actions 1` reader and the group renderer; the security review settles founder question 3. An update that changes the pack gives a new `pack_id`, which drops the KV file. |

## Changes to recently merged and open work

| Work | Follow-up change, files | Why |
|---|---|---|
| PR #15 seamless actions (merged) | In P1's `agentabi` PR (0002): `TIERS` (shellcore/src/tiers.rs:35-44) becomes row data; the cross-check test (shellcore/src/tests/actions.rs:36-50) goes; the pack's marks read the same rows. A renderer test fails if PACK or MODE text asks the model to confirm a run-tier call (today's prompt has no such text). `Done` (shellcore/src/undo.rs:20-27) gains the call's arguments as text, so it becomes an `actlog 1` record carrying `pack_id`. No revert. | One list instead of three; the pack must not teach the old confirm-first behaviour. |
| PR #10 fluency margin (merged) | P1's first pack PR renders its prompt byte for byte (golden). Fix the stale "about 650" (kabi/src/model.rs:61) with S10-0's count, and the stale gate rows docs/decisions.md:77, :152 and docs/roadmap.md:86. | It is the control arm; 47/50 carries over. |
| PR #14 faster decode (merged) | No code change. S10-3 re-measures prefill and decode (quant/src/attn.rs, attn_avx2.rs, infer/src/attend.rs). | The new attention kernels change long-context cost. |
| PR #13 parser hardening (merged) | Hostile tests in its pattern (testkit/hostile.rs) for the State builder and the group renderer, in the PRs that add them; 0003 adds the `kai-actions 1` reader's. | Both read untrusted text. |
| PR #17 terminal look (merged) | No change now. The bar (kernel/src/console/status.rs:88-116) shows model state, CPU load, memory and uptime, none of which State carries. Rule for later: a fact shown on both comes from one function. | The judge's ask assumed a shell-built status line; on main the bar is kernel code. |
| PR #16 DHCP, DNS, ping (green) | No change before merge. In P5 `:resolve` and `:ping` (kernel/src/net_commands.rs on the branch) become `net_resolve` and `net_ping` rows; State's `net` field comes from the same status `:net` prints. | Network actions join the registry like any other. |
| PR #18 virtio-gpu, PR #19 USB input (green) | No change. They feed State fields from P3. | Neither touches llmproto, shellcore or infer (checked with `git diff --stat` against main). |
| llm service (no PR) | Add `"tune"` to the re-warm match (user/src/bin/llm.rs:165). One line. | Today the next script after `tune` pays the cold prefill inside its own time limit. |
| Fluency harness (no PR) | `--seed` for `--local` (xtask/src/fluency/mod.rs:277-281); `pack_id`, format, real token count, commit and seed in the header (:406-414). S10-1's first PR. | Noise and versions become measurable. |
| Docs (after approval) | A decisions row for this ADR; the skill pack in roadmap.md:139's P0 list. | It is recorded nowhere today. |

## Open questions for the founder

1. **Memory.** A 3,072-position context costs llm 144 MiB more and needs about 288 MiB more free RAM when the model loads; the VM for the 4B grows from 3,584 to 3,840 MiB (est.). The 4B's "8 GB of RAM" recommendation stays. Accept this for v0.1, or keep 2,048 and accept that the AI sees fewer actions at once? Recommended: accept.
2. **Privacy of the live snapshot.** The AI would see the focused app, open window titles (cut to 40 characters), the current folder and the selection. All of it stays on the machine, and the pack stores none of it. Allowed in v0.1? Should a setting turn it off? Recommended: allowed, with an off setting.
3. **App descriptions in the AI's prompt.** An installed app's action descriptions (at most 60 bytes each, re-rendered by the OS) are text the AI reads. A hostile app could word them to steer the AI. Tiers stay in code, so it cannot raise its own permissions. Packages come from three places: apps we sign (through 0008's one TUF repository); apps the user builds in the terminal (0012's `cc build`), which install from 0011's `local` repository, signed by an Ed25519 device key made on this machine; and other publishers' repositories that the user adds and trusts (`pkg_repo_add`, `pkg_trust_key`, 0011). Which of them may put action descriptions in the prompt before P8's security review? Recommended: our signed apps and `local`. `local` is the user's own code, built by the user on this machine, and without it a v0.1 app made with `cc build` would install with no rows the AI can see. Other publishers' apps still install and run, but their action groups stay out of the pack until P8.
4. **Fine-tuning later.** Training Qwen3-4B on our pack format (its Apache-2.0 licence allows it) could add accuracy, but it fixes the format and costs paid GPU time (not costed here). Rule it in for after v0.1, or out? Recommended: out until S10-1 shows a gap that prompts cannot close.

No new licence question: every crate here is in the free class and is listed above. Rhai's `smartstring` (MPL-2.0, an ask-class licence) is 0009's and 0012's case. Note for them: rhai 1.27.0 is released (crates.io, 2026-10-21T13:05Z, MIT OR Apache-2.0), but crates.io still lists `smartstring ^1.0.0` as a non-optional dependency, with `compact_str` optional. So moving to 1.27.0 alone does not remove `smartstring` from the build graph (from the crates.io dependency list; the source was not read).

## Sources

External (read 2026-10-21 unless stated):
- Qwen3-4B-Instruct-2507 model card and `tokenizer_config.json` (chat template), Apache-2.0: https://huggingface.co/Qwen/Qwen3-4B-Instruct-2507
- Qwen function calling: https://qwen.readthedocs.io/en/latest/framework/function_call.html
- BFCL V4 data, leaderboard updated 2026-04-12 (rows "Qwen3-4B-Instruct-2507 (FC)" and "(Prompt)"): https://gorilla.cs.berkeley.edu/data_overall.csv; format study, 2025-07-17: https://gorilla.cs.berkeley.edu/blogs/17_bfcl_v4_prompt_variation.html; code, Apache-2.0, pushed 2026-10-17: https://github.com/ShishirPatil/gorilla
- wit-parser 0.262.0, 2026-10-13: https://crates.io/crates/wit-parser
- rhai 1.26.1 and 1.27.0 (2026-10-21T13:05Z), MIT OR Apache-2.0: https://crates.io/crates/rhai; 1.27.0's dependencies: https://crates.io/api/v1/crates/rhai/1.27.0/dependencies; `to_upper` in 1.26.1 `src/packages/string_more.rs:390`; `gen_fn_signatures` in `src/api/register.rs:737` (under the `metadata` feature)
- wassette v0.8.1, 2026-10-19, MIT: https://github.com/microsoft/wassette
- llguidance 1.9.1, 2026-09-30, MIT: https://github.com/guidance-ai/llguidance
- llama.cpp server README, MIT, b11802 (2026-10-21): https://github.com/ggml-org/llama.cpp/blob/b11802/tools/server/README.md
- vLLM prefix caching, v0.32.0 (2026-10-18): https://docs.vllm.ai/en/latest/design/prefix_caching.html; SGLang v0.5.22 (2026-10-19), Apache-2.0: https://github.com/sgl-project/sglang
- SGLang jump-forward, 2024-02-05: https://lmsys.org/blog/2024-02-05-compressed-fsm/
- TinyAgent, 2024-05-29 (Table 1, tool retrieval): https://bair.berkeley.edu/blog/2024/05/29/tiny-agent/ and https://arxiv.org/abs/2409.00608
- "Notation Matters", v2 2026-06-17: https://arxiv.org/abs/2605.29676; TOON v4.4.0 (2026-10-08), MIT: https://github.com/toon-format/toon
- xLAM-2 and Hammer2.1 licences (Hugging Face model tags, read 2026-10-21): https://huggingface.co/Salesforce/Llama-xLAM-2-8b-fc-r, https://huggingface.co/MadeAgents/Hammer2.1-7b, https://huggingface.co/MadeAgents/Hammer2.1-3b
- Cloudflare Code Mode, 2025-09-26: https://blog.cloudflare.com/code-mode/
- Anthropic, code execution with MCP, 2025-11-04: https://www.anthropic.com/engineering/code-execution-with-mcp
- Manus, context engineering, 2025-07-18: https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus
- Agent Skills specification (code Apache-2.0, docs CC-BY-4.0, per its README): https://agentskills.io/specification and https://github.com/agentskills/agentskills
- PR #10 (47/50, task 33, the reverted 46/50 follow-up): https://github.com/dane1599d/kernel-ai/pull/10

Repo (main, ec5ecff):
- Prompt and wire: llmproto/src/script.rs:24, :28, :51-94, :97-133, :136-174, :207, :405-417; llmproto/src/chat.rs:13-16; llmproto/src/explain.rs:14-20; llmproto/src/local.rs:156-160; llmproto/src/tune.rs:57-63, :215-224; llmproto/src/tune/replay.rs:37-48; kernel/src/live_commands.rs:112; kernel/src/llm.rs:142
- Engine: infer/src/chat.rs:386-411; infer/src/kv.rs:8-12; infer/src/json.rs:64; infer/src/cache_tests.rs:133; bpe/src/chat.rs:95-113
- Budget: kabi/src/model.rs:61-70, :77, :145; kabi/src/lib.rs:160; kabi/src/thread.rs:23; kernel/src/model.rs:40-57; kernel/src/process/mod.rs:58; xtask/src/disk.rs:169-175; xtask/models.json:150-152
- Shell: shellcore/src/script.rs:51-57, :61, :147, :190-191, :197; shellcore/src/commands.rs:107-113; shellcore/src/tiers.rs:35-44; shellcore/src/undo.rs:20-27; shellcore/src/tests/actions.rs:36-50; shellcore/Cargo.toml:12; user/src/bin/llm.rs:165
- Harness: xtask/src/fluency/mod.rs:277-281, :406-414; xtask/src/fluency/tasks.rs:340; .github/workflows/fluency-model.yml; .github/workflows/llama-goldens.yml
- Merged and open work: kernel/src/console/status.rs:88-116 (#17); quant/src/attn.rs, quant/src/attn_avx2.rs, infer/src/attend.rs (#14); testkit/hostile.rs (#13); origin/dane/net-dhcp-dns-b5bdb7, origin/dane/virtio-gpu-a5daaa, origin/dane/usb-input-83eb7c
- Docs: docs/decisions.md:77, :127, :148, :149, :152, :171, :174, :185, :187, :205; docs/roadmap.md:86, :127, :130, :139-147
- P0 inputs: the decision sheet and topic 10's judgement (work/10/judgement.md), designs A, B and C and the two research notes
