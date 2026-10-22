# ADR 0002: The agent action schema and autonomy tiers

Proposed, 2026-10-21. For the founder's approval; nothing here is decided until approved.

Repo citations are `path:line` on main at ec5ecff unless a branch is named. "Est." marks arithmetic from measured rates, not a measurement. "Unverified" marks what could not be read at the source.

## Context

The founder: "if i tell it to do something it should just do it." A request like "pull up my browser and go to youtube" should just happen once a browser exists (decisions.md:203). The roadmap fixes the frame: every feature declares its actions with name, arguments, risk tier and undo (roadmap.md:127); one capability model covers agent actions (:129); read-only and reversible actions run at once, destructive, outbound or irreversible ones ask, some never run, and every action is logged and undoable where possible (:130); P1 builds a registry, constrained decoding into action calls, tiers in code and an action log with `undo` (:140); tiers come from each action's declaration, never from the model, and text read from data cannot raise a tier (:187).

What is on main today:

- **The model writes a script.** It answers `{"script":"...","summary":"..."}` under a token mask (`llmproto/src/local.rs:26-50`, `infer/src/json.rs`). Scripts call seven functions in `llmproto::script::API` (`llmproto/src/script.rs:51-94`), each marked only `Read` or `Change` (`:36-39`), with no typed arguments, tier or undo.
- **Seamless actions (#15, merged).** `shellcore::tiers::TIERS` puts all seven at `Run` (`shellcore/src/tiers.rs:35-44`); an unlisted function asks (`:47-52`); a script takes its strictest function's tier (`:55-67`). A run-tier script runs at once and replies `` Done: policy round-robin (was interactive) (undo: `undo`) `` (`shellcore/src/actions.rs:128-139`). `undo` is one slot deep (`:31-34`, `:111-118`), and `revert` reinstalls the old policy without checking that nothing changed since (`shellcore/src/undo.rs:48-55`).
- **Gaps.** Nothing in code notices a change the model adds to a read: the fluency judge checks it for read tasks only (`xtask/src/fluency/judge.rs:61-65`). `names_in` counts names inside comments and strings (`shellcore/src/script.rs:197-211`). Two `policy_name` functions disagree (`shellcore/src/script.rs:51-57`, `shellcore/src/commands.rs:107-115`). There are three undo stores: `tune undo` (`shellcore/src/tune.rs:68`), the script slot, and `:revert` with 6 versions (`kabi/src/live.rs:211`). Logs live in memory and are lost when the shell restarts (`shellcore/src/log.rs:9`).
- **Who can act.** The shell answers only caller 0, the kernel terminal (`user/src/bin/shell.rs:142-147`). The `llm` process holds no POLICY or CALL bit (`kernel/src/llm.rs:74-80`). The model never sees what a script reads. Each request is one shot: plan first, then run.
- **Decoder and context.** Fields are strings or choices of at most 64 words (`infer/src/json.rs:20-25`, `:64`) in one fixed order. Every decoded token, forced bytes included, costs one `forward` step (`infer/src/chat.rs:325-375`, `:373`); only prompts use `forward_batch` (`:402`). The context is 2,048 positions (`kabi/src/model.rs:65`); [ADR 0010](0010-skill-pack.md) raises it to 3,072 from P1.

What has been measured, and where:

| What | Number | Where |
|---|---|---|
| Fluency gate, real Qwen3-4B Q4_K_M | 47/50 first try and after repair; median 18.5 s per task | GitHub runner (ubuntu-24.04, 4 vCPU, AVX2, no AVX-512), host engine, run 39675848260 (2026-10-21) |
| "Switch the scheduler to round robin." (script and summary) | 21,713 ms | same run, task 41 |
| One-sentence refusals, no script | 7.8-9.1 s | same run, tasks 46-50 |
| Prefill, generated weights in Qwen3-4B's shapes | 52 tokens/s on 4 threads, 91 on 8 | 7700X host threads, dev build (decisions.md:174) |
| Decode, same weights | 11.5-12.9 tokens/s on 4-8 threads (decisions.md:171); 14.6-16.2 in a later run (:174) | 7700X host threads |
| VirtualBox guest decode | 11-12 tokens/s; the row does not name the model | founder's PC (decisions.md:211) |
| Guest with the real 4B, tokens per answer, mask cost per step | not measured | none |

The runner's own tokens/s is not logged. The real model never runs on the founder's PC (decisions.md:185).

## Options considered

Three designs were written and judged on 2026-10-21. A fourth axis, open-source reuse, is in the second table.

| Design | What it is | Evidence for and against | Score | Outcome |
|---|---|---|---|---|
| **A. Typed registry, request as consent** | One declaration per action. The model emits one tagged call under constrained decoding; a script is one more action. Code derives the tier and checks the call against the user's line. | For: the fewest decoded tokens from Enter to done; the result line is written by code; one versioned declaration drives prompt, binding, tier and undo, so no row can be run-tier without an inverse; matches roadmap.md:127, :140, :187 word for word. Against: the decoder work (tagged shape, nesting, integers, jump-forward) is the largest and riskiest piece; call accuracy is unmeasured; lexical consent is brittle; its script path is thin. About 4,500 lines, 10 PRs in P1. | 8 | **Chosen as the base.** |
| **B. Scripts with effect analysis** | The model keeps writing Rhai. Code walks the syntax tree, labels every value (Literal < System < Content), derives tiers from effects and runs each script as one transaction. | For: the most rigorous provenance and the all-or-nothing transaction, needed anyway for multi-step requests; lowest decoder risk; about 3,500 lines. Against: every request still decodes free text, and a failed compile or trial costs a whole repair round (task 19 took 50.4 s); its "not asked for" rule had a hole: "Tell me the number of processors" plus a model-added `set_policy` matched no rule and ran; rollback did not handle `Busy` within 100 ms of a switch (`kernel/src/sched_control/calls.rs:16-35`); reinterprets roadmap.md:140. | 7 | **Lost as the base; its analyser and transactions are grafted on.** |
| **C. MCP-shaped actions** | Every action is an MCP `Tool` with JSON Schema arguments and annotations; the model answers in Qwen's `<tool_call>` form; providers speak JSON-RPC over IPC from P3. | For: best reuse (rmcp as an oracle, the conformance suite); a public vocabulary. Against: Qwen's JSON tool block for today's 7 functions is 1,364 bytes against 553 for today's lines (2.5 times; C's count, the 553 rechecked by the judge) in a 2,048-position context; ties the app ABI at P3 to a spec revised four times from 2025-03-26 to 2026-07-28 whose tools page says clients SHOULD confirm operations; every MCP request carries `_meta` bytes against an 8,192-byte message (`kabi/src/lib.rs:160`); the interop payoff arrives at P5/P6 at the earliest, since roadmap.md:128 keeps the AI local ("There is no cloud AI"); keeps `names_in`. About 4,100 lines plus about 1,500 for IPC. | 6 | **Lost; its naming, row consistency checks and P5 bridge plan are grafted on.** |

Also weighed and not taken: a second model call that judges "did the user ask for this" (Anthropic measured 17% of real over-eager actions missed by its classifier pipeline, and a second decode costs seconds here); scripts as the only form (kept as the fallback if spike S02-1 fails); JSON-RPC between first-party processes in v0.1.

Open-source reuse, licences checked 2026-10-21:

| Project, version | Licence | What we take | Outcome |
|---|---|---|---|
| MCP specification 2026-07-28 | Apache-2.0 for new code and spec text, MIT for older code, CC-BY-4.0 for docs | The four `ToolAnnotations` hints, their meanings and pessimistic defaults, as the `facts` field names | Taken (vocabulary only, no code) |
| rmcp 3.6.0 (2026-10-20), the official Rust SDK | Apache-2.0 | Host-side conformance oracle for a P5/P6 bridge, in xtask only | Deferred to P5; free class, listed in that PR |
| FIDES (microsoft/fides), arXiv 2505.23643 v2 | MIT | The two policies P-T and P-F; typed quarantined reads | Taken as ideas (the repo is one notebook) |
| CaMeL (google-research/camel-prompt-injection), arXiv 2503.18813 v2 | Apache-2.0 | Labels on values checked before each call; plan before data | Ideas only; its Python interpreter is unmaintained |
| AgentDojo 0.1.35 (ethz-spylab/agentdojo) | MIT | Test structure (user task, injection placed in data, check the injected action never ran) and its strings, with attribution | Taken for boot cases and S02-1 |
| BFCL (ShishirPatil/gorilla) | Apache-2.0 repo; data licence unverified | Category names `simple`, `multiple`, `irrelevance` | Names only; no data copied |
| Qwen3-4B-Instruct-2507 chat template | Apache-2.0 | `name`/`arguments` key names; `<tools>` rendering as S02-1's second arm | Taken; whether it helps is measured, not assumed |
| Rhai 1.27.0 (2026-10-21), with rhai_codegen 3.3.0 | MIT OR Apache-2.0, both; Rhai is already a dependency (1.26.1 today; ADR 0012 D9 pins `=1.27.0` at the start of P1) | `internals` feature: `AST::walk` (`src/ast/ast.rs:780-787` at tag v1.27.0), custom types for `Data` | Taken; the feature is `internals = []` (`Cargo.toml:68`), so no crate is added |
| llguidance 1.9.1 (2026-09-30) | MIT | A general grammar engine | Not taken: `std` throughout; extending `infer/src/json.rs` is cheaper for tens of rows |
| outlines-core 0.2.14 (2026-01-09), XGrammar | Apache-2.0 | Same | Not taken: `regex` and per-token indexes; C++ |
| Wassette `component2json` | MIT (version unverified) | WIT to JSON Schema | Not needed here; [ADR 0003](0003-app-model.md) extracts `kai-actions 1` itself |
| GoEX (gorilla `goex/`) | Apache-2.0, dormant since 2024-07 | Per-action inverse table | Idea only; model-written inverses are refused for the run tier |
| Snapper | GPL-2.0 | Pre/post snapshots around changes | Idea only; no code |

Rhai, rhai_codegen, rmcp, llguidance and outlines-core versions, dates and licences were rechecked on crates.io on 2026-10-21; the other licences on each project's GitHub LICENSE. No GPL, LGPL or MPL code enters through this ADR. Rhai's existing `smartstring` dependency is MPL-2.0+; see question 10.

## Decision

In plain words: every feature lists the actions the AI may use, each with its arguments and how to undo it, if it can be undone. When you type a request, the AI picks one of those actions, and code, not the AI, decides whether it runs. What you plainly asked for and can undo runs at once and says what it did, with `undo`; anything the AI added on its own, anything with no undo and anything that sends your data out asks you first. Every action is logged, and `undo` never overwrites a change you made since.

In design terms: base design A, with B's script analyser and transactions and C's naming, consistency checks and bridge plan grafted on. Shared names, budgets and owners follow the P0 decision sheet.

### D1. The registry: crate `agentabi`, `REGISTRY_VERSION = 1`

A new `no_std` crate. One row per action, a `const` for each OS action. `llmproto::script::API`, `TIERS`, the Rhai bindings, `:help` and the prompt rows are generated from it, which also removes the second `policy_name`.

```rust
pub const REGISTRY_VERSION: u32 = 1;
pub struct Action {
    pub name: &'static str,             // <group>_<verb>, snake_case, at most 24 bytes, a valid Rhai and C identifier
    pub doc: &'static str,              // one line for the model, at most 60 bytes, so a dsl-1 line fits ADR 0010's 120
    pub args: &'static [Arg],
    pub facts: Facts,                   // read_only, destructive, idempotent, open_world (MCP's hints)
    pub inverse: Inverse,
    pub cues: &'static [&'static str],  // request stems that ask for it: "polic", "schedul"
    pub returns: Returns,               // System, or Content(source): what a read hands back
    pub ret: RetType,                   // the result's shape: #{free_mib: int, total_mib: int, ...}
    pub owner: Owner,                   // Kernel | Service(name) | App(id): who executes it and its inverse
    pub needs: Needs,                   // handle kind and rights (ADR 0001)
    pub min_tier: Tier,                 // Run, Ask or Never: the derived tier is at least this
    pub old_name: Option<&'static str>, // kept for one REGISTRY_VERSION after a rename
}
pub struct Arg { pub name: &'static str, pub ty: ArgType, pub role: Role, pub from_request: bool,
                 pub never: &'static [&'static str], pub aliases: &'static [(&'static str, &'static str)] }
pub enum ArgType { Enum(&'static [&'static str]), Int { min: i64, max: i64 }, Text { max: u16 }, Path, Host,
                   List(&'static ArgType, u8), Opt(&'static ArgType) }
pub enum RetType { Unit, Of(ArgType), Record(&'static [(&'static str, RetType)]), List(&'static RetType), Data }
pub enum Role { Target, Payload, Option }
pub enum Inverse { Read, Restore, Compensate(&'static str), Snapshot, Trash, Hold { ms: u32 }, None(&'static str) }
```

- **Facts** use MCP's pessimistic defaults: not read-only, destructive, not idempotent, open-world. A `const` check refuses an inconsistent row: read-only with an inverse other than `Read`; not destructive with `None`; closed-world with a `Host` argument; a `never` value that is also an alias.
- **`needs`** replaces the judgement's `cap` field: [ADR 0001](0001-capabilities.md) turns today's bits (`kabi/src/lib.rs:82-108`) into kind-ceiling bits and handles. **`min_tier`** is the judgement's `ceiling`, renamed so it does not clash with 0001's kind ceiling.
- **`ret`** is the result type, for results only: the `ArgType` set plus records, lists of them and `Data`. ADR 0010's `dsl-1` renders it (`memory() -> #{free_mib: int, total_mib: int, heap_kib: int}`), so a script knows `memory().free_mib`, and ADR 0012's `kai.h` lowers it to structs. `returns` keeps the provenance label. A `Content` row's result is `Data` or holds it; the consistency check refuses one whose `ret` does not.
- **`Inverse`** is the declared kind. The runtime record stays a small `Copy` enum in the shell (`shellcore/src/undo.rs:13-18`) and gains one variant per owner: `Spec` today, `Fs { txn }` ([ADR 0004](0004-storage.md)), `Slot { release_seq }` ([ADR 0008](0008-updates.md)), a generation for packages ([ADR 0011](0011-packages.md)).
- **Names.** Apps use `<app>_<function>` ([ADR 0003](0003-app-model.md)). Scripts call the registry name as it is, never a module path; shellcore builds Rhai with `no_module` (`shellcore/Cargo.toml:12`). No dots anywhere.
- **The seven M8 names** (`cpus`, `memory`, `uptime_ms`, `processes`, `threads`, `get_policy`, `set_policy`) stay in `REGISTRY_VERSION = 1`. A rename (for example `set_policy` to `sched_set_policy`) lands only when the fluency gate, run through `script` on a GitHub runner with the new names, scores at least 45/50 and no more than one task below the old names, at seed 42 and two more seeds. The row then keeps `old_name` as a second binding for one version, so logged scripts and typed habits still work, and `REGISTRY_VERSION` becomes 2. Argument keys keep their names too: `set_policy` takes `name` (`set_policy(name: string)`, `llmproto/src/script.rs:90`), as ADR 0010 renders it. Changing a key rides the same gated rename: at least 45/50 on a runner, and the old key kept for one version.
- **Arguments** are limited to the types above, the ones the decoder can steer. ADR 0003's WIT exports map onto them; anything else does not register.
- **Where rows come from.** OS rows are `const` in `agentabi`. App and package rows are read from the artifact's `kai.actions` section (`kai-actions 1`, ADR 0003) into the same fields with owned strings, pass the same consistency check when loaded, and join or leave the registry with the package generation (ADR 0011).

### D2. The tier: derived in code, never chosen

`decide(call) = max(derive(row), consent(request, call), labels(call))`, checked deny, then ask, then run; the first match wins, as in Claude Code's permission rules. A script takes its strictest call, as today's `of_script` does (`shellcore/src/tiers.rs:55-67`).

| `derive(row)` | Tier |
|---|---|
| Read-only, closed-world | Run |
| Read-only, open-world (a fetch) | Run if the host is grounded (D3), else Ask |
| A change whose inverse is `Restore`, `Compensate`, `Snapshot` or `Trash` | Run |
| `Hold { ms }`: an outbound send held with `undo` live (P5) | Run only if question 3 says so; Ask until then |
| Inverse `None`, or an open-world change with a `Payload` (data going out) | Ask |
| A `never` value, `min_tier = Never`, a name outside the registry | Never (refused; the model is told once) |

`min_tier` only raises. Nobody writes a tier by hand, so a new row cannot be run-tier without an inverse, and every run-tier row needs a test that its undo restores. Today's never-tier precedents become rows: `PROTECTED` targets (`shellcore/src/tune.rs:39`, `kernel/src/sched_control.rs:30`) and changes to the registry, tiers, log, snapshots or grants. Typed lines are the user's own: a typed call goes through the same door with consent `typed`, so it is logged and undoable, and never asks (typing is consent, as `:revert` already treats it, `kernel/src/edit_mode.rs:187`); never-tier rows still refuse.

An ask-tier call gets a `ONCE` handle from ADR 0001, minted by the trusted path on the user's `y`, with a badge digest over the entry id, the action name and the arguments exactly as the question showed them. A never-tier call gets no handle.

### D3. Consent: the user's line is the permission

The rule is Anthropic's for Claude Code's auto mode: "The prompt establishes what is authorized; everything the agent chooses on its own is unauthorized until the user says otherwise." It is code with unit tests per row, not a model.

- A read-only, closed-world call is always consented.
- A change call is **asked for** only when (a) one of its `cues` starts a word of the user's line, not within three words after `don't`, `not`, `never` or `no`; and (b) every `from_request` argument is literal in the line after folding case, spaces and hyphens ("round robin" is `round-robin`), or is a declared alias, or is one of ADR 0010's symbols `focused`, `selected`, `here` when the line says "this", "it", "here" or "selected" (the OS resolves those from system state, labelled System).
- A `Host` is **grounded** only if the user typed it, or it is the entry for a request word in a curated host table shipped with the registry ("youtube" to `www.youtube.com`), or the user said yes to it before (a grant record under ADR 0001). A label match is not enough: `youtube.xyz` and `youtube.ru` would pass it.
- A host on the OS's own source list is also grounded, when the bytes fetched from it are checked against signed metadata before use: the TUF repository `dane1599d/kernel-ai-updates` and its Pages host (ADR 0008), the pinned Hugging Face revision for models, and the Debian mirrors for ADR 0006's layer (apt's signed release files). The user's line need not name it. So ADR 0008's `model_fetch` derives Run (undo deletes the new blob), with `min_tier = Ask` only if 0008's question 3 says large background downloads ask; ADR 0006's `linux_upgrade` and ADR 0011's `pkg_install` and `pkg_upgrade` (P5) are grounded the same way.
- Anything else is **added**, and it asks, whatever else the plan contains: `You asked for X. It also wants to set the policy to round-robin. Do that too? y/n, or show`.
- `clarify` asks the model's question; the reply is appended to the original line and the plan is redone, since both are the user's words.
- Plainly asked and undoable: runs at once. Plainly asked with no undo: asks once (question 1). An indirect request ("make it snappier") has no cue, so it asks until question 4 says otherwise. A package the user's own line names installs without asking when no new grant choice is needed (generation undo); this departs from the brief's "ask" and is ADR 0011's question.

### D4. What the model emits: one tagged call

Every answer is `{"name":"<action>","arguments":{...}}`, the key names inside Qwen3-4B-Instruct-2507's `<tool_call>` template. `name` is a choice over registry names plus three pseudo-actions: `say(text)` (at most 240 bytes; replaces the empty-script reply, `llmproto/src/script.rs:349-371`), `clarify(question, options)` (at most 6 options) and `script(code, lang?)` (today's limits: 40 lines of 120 characters, 3 KiB, `llmproto/src/script.rs:19-21`). Once `name` closes, `arguments` takes that row's fields in declared order.

- **`lang`** is `rhai` (the default) or `c`. `c` stays masked out of the grammar until ADR 0012's C fluency gate passes on a runner (70% first try, 90% after one repair); a `c` script then goes to 0012's `cc`. Same tiers, transaction and log either way. ADR 0012 D8 asked for this field; this ADR owns it.
- **No model summary.** Code writes the result from the log: `Done: policy round-robin (was interactive). undo`. This replaces the `{script, summary}` envelope from P1 and drops up to 100 characters of decoding (`SUMMARY_MAX_CHARS`, `llmproto/src/script.rs:23`). #15's behaviour (plain requests run at once, `undo`, `show`) is kept and generalised.
- **Decoder work** in `infer/src/json.rs` (604 lines): a tagged shape (first field a choice, each word selecting the next fields), one nested object, bounded `Int`, `List` and `Opt`, and a name choice past 64 words. **Jump-forward** in `infer/src/chat.rs`: when the grammar allows exactly one continuation (`","arguments":{"name":"` after `set_policy`), its tokens are fed as one `forward_batch` block instead of one decode step each (SGLang's jump-forward). Keys and values at a position depend only on the tokens up to it (decisions.md:148), so the cache stays exact; the tokens fed may differ from what step decoding would pick, which S02-1 measures.
- **Prompt.** One compact line per row, rendered by ADR 0010's `dsl-1`. Qwen's `<tools>` JSON replaces it only if S02-1 shows it at least 3 points more accurate within the budget, and by more than the spread between seeds. The plain `ask` mode moves behind the same pack prefix so the one prefix cache stays warm.
- **Budget** (`CONTEXT_CAP` 3,072, owned by ADR 0010): core rules plus OS rows at most 1,024 tokens; app groups at most 768; State line at most 64; request at most 160; answer reserve 1,024.

### D5. Scripts: analysed, isolated, all or nothing

`script(code, lang?)` keeps the strict profile (`shellcore/src/script.rs:173-194`) and the trial run on stub values. What follows is for `rhai`; a `c` script gets ADR 0012's run-level label instead, since a C value cannot carry `Data`.

- **Analyser.** B's AST walk replaces `names_in`, with Rhai `internals` enabled and the version pinned (`=1.27.0`; the book calls internals "volatile"). ADR 0012 D9, which owns the Rhai upgrade, moves to 1.27.0 at the start of P1, so the analyser is written once against it. Each call becomes an effect with labels Literal < System < Content, joined through `let`, assignment, indexing, `if` and `for`; anything unproven is Content. A name in a comment no longer triggers a question.
- **Isolation.** The script runs in ADR 0001's `scriptrun` process, holding only run-tier handles narrowed under the request's grant node. If 0001's S01-1 says a pooled `scriptrun` is still too slow, scripts stay in `shell` under request nodes. ADR 0001's `user` label is Literal or System here; its `data` label is Content.
- **One transaction per script.** Snapshots are taken lazily at the first change to each resource. Every live call is re-checked against the approved effect set: the same action, the same literal arguments, System-derived values inside the resource's scope. Any error, limit or Ctrl-C rolls back newest first and replies `Nothing changed: <error>`, replacing "Changed before the error" (`shellcore/src/actions.rs:133-137`). Outbound effects are queued and flushed only on success (the transactional outbox pattern).
- **One door.** Tagged calls, script host functions, typed calls, ADR 0003's queued app effects and ADR 0012's `cc` need/resume all reach `agentcore::execute(call, consent) -> Outcome`. `Outcome` is structured (tier, consent, done records, undoable), so the P3 panel ([ADR 0005](0005-graphics.md)) reads states, not text.

### D6. Data labels: what read data cannot do

A read whose row returns `Content(source)` hands back an opaque Rhai custom type `Data`, with no `to_string`, comparison or conversion registered. In Rhai 1.27.0, as in 1.26.1, `string + item` formats through `to_string` (`src/packages/string_more.rs:24-29`) and `Display` of a custom type writes only its type name (`src/types/dynamic.rs:563`), so content cannot become a plain value. How `==` behaves between `Data` and a string is unverified; either way it carries no content. `show(d)` prints it to the user.

- **P-T (integrity).** A `Data` value in the Target or Payload of a call whose inverse is `None` or `Hold`, or controlling such a call, makes it ask, and the question names the source (`the target came from note "todo"`).
- **P-F (confidentiality).** A `Data` payload to an open-world call goes out only to a destination the user's line names; otherwise it asks. Values labelled secret (P8 accounts) never leave.
- Undoable local calls may take a consented `Data` target ("move the biggest file in Downloads to Old" runs; `undo` puts it back).
- ADR 0010's State line, [ADR 0006](0006-existing-software.md)'s guest page text and anything an app returns as content are `Data`. Data can never start an action, add one, lower a tier, or pick the target or payload of an undo-less or outbound action without a question that shows it. From P5, a read the model must reason over goes to a separate quarantined request whose output is a bool, an enum or a short string, returned as `Data` (FIDES's typed `query_llm`; it fits the existing `Choice` decoder).

### D7. The action log and undo: `actlog 1`

One request is one entry, a saga. Each executed call appends a `D` line with owner, resource key, before, after and inverse. Text, one record per line, length-prefixed where text is free; argument values in ADR 0003's WAVE value text.

```
actlog 1
E 17 t=1792618800123456789 reg=1 pack=9f3c0a1e5b7d2c44 form=call tier=run consent=asked state=done
R 21
switch to round robin
D 17.1 set_policy owner=kernel res=policy before=interactive after=round-robin inv=restore
U 18 of=17 state=done
S 19 t=1792618860000000000 owner=kernel 33
nvme0 reset after 5 s timeout, ok
```

- **System events.** An `S` record (`S <id> t=<ns> owner=<service> <length>`, then the text) logs what no request caused: a driver reset (ADR 0007 §6), a trash empty or snapshot expiry (ADR 0004), a package GC run (ADR 0011). The agent can read them; `undo` skips them. The record is in `actlog 1` from P1, before the log reaches disk in P2, so the format does not change then.
- **Where.** 64 entries in memory in P1. From P2, appended to `/system/log` in the same commit as the action (ADR 0004). `pack=` is ADR 0010's `pack_id`; `reg=` is the registry version, so entries stay readable across updates (ADR 0008).
- **`undo`** reverts the newest live entry, its calls newest first. Each inverse is compare-and-set: it runs only if the resource still reads `after`. If it moved, scalar state asks (`The policy changed since (now classes/firm); put interactive back anyway? y/n`) and files are restored beside the changed path as `name (restored)` (ADR 0004). Undo never silently overwrites a later change.
- An undo is itself an entry, as in Emacs, so `undo` again redoes. `undo N` runs only if no later live entry touched the same resource key; otherwise it names those entries. `Busy` from the 100 ms policy limit is retried once after 100 ms.
- The owner executes the inverse with its own handles; the agent core only routes it, so undo works across apps. `tune undo` and the script slot fold into this log; `:revert` is registered as a row.
- **When undo is impossible**, the question said so before the run (inverse `None`). Afterwards the entry is marked `stuck:<reason>` and the reply says `cannot undo: <reason>` in code's words: the owner is gone, a snapshot expired, or the user said no to a moved resource. Undo snapshots are pinned by their log entry for 30 days or until free space falls under 10% (ADR 0004).

### D8. Time from typing to done

Single plain call, warm pack in the prefix cache. Rates are measured on the 7700X host with generated weights (decisions.md:171, :174); token counts are estimates until S02-1 counts them; guest numbers wait for S02-2.

| Step | Tokens (est.) | 7700X, 4-8 host threads (est.) | Budget |
|---|---|---|---|
| Enter to the busy sign, by code | 0 | under 10 ms of work | 100 ms |
| Prefill State line and request (pack cached) | 40-90 | 0.45-1.7 s at 52-91 tokens/s | State 64, request 160 |
| Free decode: name, argument values, close | at most 12 | 0.75-1.05 s at 11.5-16.2 tokens/s | 12 tokens |
| Forced bytes as one block (jump-forward) | about 12 | 0.13-0.23 s at prefill rates; 0.75-1.05 s as decode steps | none |
| Consent, tier, execute, log, result line | 0 | milliseconds (unmeasured) | 50 ms |
| **Total** | | **about 1.3-3.0 s** | target about 2-4 s under KVM, set by S02-2 |

In a guest, decode near VirtualBox's 11-12 tokens/s (decisions.md:211, model not named) puts a call at the slow end of the range. Today the same request takes 21.7 s through a script on a 4-vCPU runner, a slower machine than the 7700X, so the two are not directly comparable. A `script` answer stays the slow path. Mask cost per step is unmeasured.

### D9. MCP later, as a mapping

Field names follow MCP, so P5/P6 can add an MCP client and a bridge for third-party tools whose hints only tighten tiers (the spec says clients MUST treat annotations as untrusted unless they come from trusted servers). rmcp 3.6.0 serves as a host-side oracle then. No JSON-RPC between first-party processes in v0.1. Whether to build the bridge is question 7.

## Consequences

- A plain, undoable request runs with no card and one code-written line, est. 1.3-3.0 s on 7700X host threads. Today the same request took 21.7 s through a script on a 4-vCPU runner; the two are not directly comparable, and S02-1 compares both on one runner. Questions come only for added actions, actions with no undo or that send data out, ungrounded hosts, `Data` steering a risky call, and real ambiguity.
- One row makes a feature AI-operable: prompt line, decoder choice, binding, tier and undo. P2-P4 add rows, not mechanisms.
- One log and one undo replace three undo slots and the script log. Undo is compare-and-set, so it cannot overwrite the user's later change.
- Prompt injection is held by data flow, not a detector: the plan is fixed from the user's line before any data is read, and `Data` cannot reach the choices that matter. Detectors are weak ground: adaptive attacks beat 12 published defences, most at over 90% success (arXiv 2510.09023).
- Costs: about 4,500 lines in P1 before P2 starts; a bigger decoder, a parser of model output, which parser hardening must cover; hand-written inverses, so every run-tier row needs an undo test; lexical consent that is English-only and will ask more than it should until aliases grow; Rhai becomes the long-term plan language, `internals` included.
- roadmap.md:144 ("network actions are in the ask tier") and roadmap.md:141 ("move and overwrite ask, with undo") change if questions 2 and 9 are approved. decisions.md:82 is superseded.

## What it costs to change later

| Item | Cost | When it becomes expensive |
|---|---|---|
| `derive`, consent rules, aliases, host table, budgets | Cheap: code and tests; the boot cases and the gate catch regressions | Never; they stay code |
| Action names and the call shape | Moderate: a prompt change and a gate re-run (about 22 minutes on a runner, as run 39675848260 took) | When third-party prompts or scripts depend on them |
| `actlog 1` | Moderate: versioned, so readers keep v1 | From P2, when entries reach disk |
| Row fields and their meaning | Cheap before v1.0 (every app is first-party, one sweep); new fields with pessimistic defaults stay cheap | After v1.0, once third-party apps ship `kai-actions 1` |
| `Data` labels | Cheap now; expensive to add after apps return plain strings | From the first data-returning row, so `Data` lands in P1 with `note_read` |
| Rhai as the plan language | Expensive: a new prompt, gate and analyser (about 1,000 lines), and old logged scripts must stay readable | Already partly so; deepens with every app |
| Moving the agent core out of `shell` into an `agent` service | One M-sized PR over the existing text wire | When the P3 panel needs it |

## Spikes that settle it

None runs a real model on the founder's PC. Real-model runs use the host engine on a GitHub runner, as `fluency-model.yml` does (decisions.md:185), or the OS in a KVM guest on a GitHub runner.

**S02-1, action gate** (`spikes/act-gate`, not merged; one harness with ADR 0010's S10-1, beside ADR 0001's host-only tier corpus S01-2). Build: a prototype tagged decoder with jump-forward, a mock registry of 40 rows (the 7 real ones plus stubs for P1-P5 levers, `browser_open` included), the consent code and B's analyser, host-side in xtask beside `fluency --local`. Tasks (150): 60 single plain, 20 multi-step (expect `script`), 20 ambiguous, 20 irrelevant (expect `say`), 20 over-eager bait (including ungrounded hosts), 10 negations; plus the 50 fluency tasks through `script`. Two renderings: `dsl-1` lines and Qwen `<tools>` JSON. Real Qwen3-4B Q4_K_M, seeds 42, 1 and 2, on the 4-vCPU GitHub runner.

| Measure | Pass | Fail means |
|---|---|---|
| Exact call and arguments, 60 single | at least 57 (95%) | under 54 (90%): v0.1 keeps `script` as the only form; D2, D3, D5-D7 stand |
| False runs (added, negated, bait, ungrounded host), 30 probes, every seed | 0 | fix consent before it merges |
| False asks on the 60 plain requests | at most 6 | widen aliases, re-run |
| Ambiguous requests that clarify or ask | at least 16 of 20 | add `clarify` examples |
| Free decoded tokens per single call, median | at most 12 | shorten argument shapes |
| System prompt with 40 rows, real tokeniser | at most 1,024 tokens | per-app row groups (ADR 0010) |
| Enter to done, single call against the same request as a script, same runner | call median at most 0.5 × script median | profile before P1's call path merges |
| Fluency through `script` after prompt changes | at least 45/50 | revert the prompt changes |
| Soundness: every live call is in the static effect set | 100% | analyser bug; blocks D5 |

Thresholds are relative or token-based on purpose: on the same runner a one-sentence refusal already took 7.8-9.1 s, so an absolute seconds bar there would measure the runner, not the design.

**S02-2, in-guest latency** (one KVM job on a GitHub runner, shared with ADR 0010's S10-3). The real model in the guest; log prefill and decode tokens/s, mask cost per step in µs, tokens saved by jump-forward, Enter to busy sign, and Enter to done for 10 single calls. Pass: busy sign within 100 ms; mask at most 2% of a decode step. The median single call confirms or replaces D8's 2-4 s target; over 4 s means a profile before the P1 call path merges, not a design change.

**S02-3, undo and rollback** (mock model, boot cases on KVM and TCG in GitHub Actions, TCG locally in WSL2). Cases: compare-and-set after a typed `policy` switch; rollback of a script that switches and then throws; undo within 100 ms of a switch (`Busy` retry); `tune` keep, then `undo`. Pass: every case restores, and none overwrites a later change.

## Phases that depend on it

| Phase | What it needs from this ADR |
|---|---|
| P1 agent core | All of D1-D8: `agentabi`, consent, tagged decoder and jump-forward, prompt from the registry (with ADR 0010), `scriptrun` and handles (ADR 0001), in-memory log and undo, `Data` with `note_read`. Rows: the seven M8 functions, `tune`, service start/stop/restart, component apply and revert, editor open at a line, process stop (ask), `system_about` ([ADR 0009](0009-licence.md)), `device_list`, `device_restart`, `device_disable`, `device_enable`, `power_reboot`, `power_off` ([ADR 0007](0007-drivers.md) §11), `update_check`, `update_status`, `update_stage`, `update_restart`, `system_slots`, `system_rollback`, `model_use`, `model_fetch` (ADR 0008 §11), `pkg_*` rows (ADR 0011). Boot cases `act-plain`, `act-run`, `act-ask`, `act-undo`, `act-rollback`, `act-inject`, `act-never` with mock answers keyed by request; the action gate (S02-1) on a runner. |
| P2 files and storage | `Snapshot` and `Trash` inverses through ADR 0004 transactions; `/system/log` on disk; file rows. |
| P3 desktop | `ui_windows`, `ui_view`, `ui_open`, `ui_focus`, `ui_act`, `window_close`, `appearance_set` (ADR 0005; `window_close` is one row for native and Linux windows, shared with ADR 0006); unbound UI nodes ask; the panel shows the result line, an Undo button and inline answers from `Outcome`. |
| P4 built-in apps | Rows from each app's `kai-actions 1` (ADR 0003) as `<app>_<function>`; app groups within 768 tokens; per-app action tests driven by the agent. |
| P5 networking | Open-world rows, the host table, `Hold`, P-F, quarantined reads; `:resolve` and `:ping` become read rows; online `pkg_install`; an MCP client if question 7 says so. |
| P6 existing apps | The rows in ADR 0006 item 7, at the tiers that table derives, acting on the Linux guest in its micro-VM: `vm_start`, `browser_open` (a named site), `browser_read` (returns `Data`), `window_close` (Linux windows), `media_play`, `media_pause`, `app_open`, `app_close` and `linux_upgrade` run with undo; `browser_type`, `browser_submit`, `browser_download` and `linux_run` ask (inverse `None`). Guest text is `Data`. |
| P7 real hardware | No schema change; device rows declare like any other. |
| P8 production | Fuzzing of the call reader, analyser and `actlog` reader; a security review of consent; per-account logs and the secret label. |

## Changes to recently merged and open work

- **#15 seamless actions (merged).** One follow-up PR before P1, about 150 lines with tests:
  - `shellcore/src/undo.rs:13-18`, `:48-55`: `Inverse::Spec` also records the spec after the change (still `Copy`). `revert` reads the current spec (`current_spec`, `:34-40`) and returns "moved" if it differs, and `shellcore/src/actions.rs:221-243` then asks `The policy changed since (now X); put Y back anyway? y/n`. A `Busy` is retried once after 100 ms through `Host::sleep_ms` (`shellcore/src/lib.rs:78`). Why: today `undo` silently overwrites a typed `policy` switch, and P1 would inherit it.
  - `shellcore/src/actions.rs:52-96`: an interim consent check for `set_policy`: it asks unless the request contains `polic`, `schedul`, `round robin`, `round-robin` or `interactive`. Why: today a read request whose script also switches the policy runs at once, which breaks "ask only for what the model adds".
  - `shellcore/src/tiers.rs:16`: `Ask` reads "Runs only after a yes: it has no undo, the model added it, or it brings new code or grants". ADR 0011 lists the same rewording, ADR 0001 adds `fixed` and `floor` rows to the table, and ADR 0003 changes `of_script` (`:55`) to take names that are not `'static`; all can share this PR.
  - Keep `Tier`, strictest wins, `show`/`why`, bare `y`/`n` and the `run script N? y/n> ` string xtask parses (`kernel/src/terminal.rs:87`, `xtask/src/qemu.rs:353-357`); P1 reuses it with N the log entry id.
  - Docs: mark `docs/decisions.md:82` superseded by :203 and :205; fix the stale "runs it only after a yes" in `llmproto/src/local.rs:11-12`, `docs/roadmap.md:84` and `:112`.
  - P1 then replaces `TIERS` with derived tiers and the one-deep `Undoable` with the log.
- **#14 faster decode (merged).** A follow-up PR adds jump-forward in `infer/src/chat.rs:325-375`, feeding grammar-forced runs through the `forward_batch` path (`:402`), with a warm-against-fresh equivalence test, and logs decoded and forced token counts per answer. Why: it is the largest latency lever for calls and already helps today's envelope.
- **#17 terminal look (merged).** A follow-up PR makes `console::status::set_busy(true)` (`kernel/src/console/status.rs:41-47`) paint the busy sign at once or wake the status thread. Why: the thread looks every `TICK_MS` = 125 ms (`:21`), so the sign can appear up to 125 ms after Enter (`kernel/src/terminal.rs:180`), over the 100 ms budget (not measured). P1 changes the result line to `Done: X (was Y). undo`, still a `= ` reply row for `termcore/src/style.rs:18`. Questions stay at most two lines so `fits_on_screen` (`kernel/src/terminal.rs:235-239`) holds at the new font size.
- **#13 parser hardening (merged).** Each P1 PR that adds a parser adds its hostile tests with `testkit/hostile.rs`: the call reader, the tagged decoder's state machine, the AST analyser (fuzz property: every live call is in the static set) and the `actlog` reader.
- **#16 DHCP and DNS (open, `origin/dane/net-dhcp-dns-b5bdb7`).** No change. `:resolve` and `:ping` stay typed kernel commands; typing is consent. In P5 they become open-world read rows.
- **#18 virtio-gpu (open, `origin/dane/virtio-gpu-a5daaa`).** No change.
- **#19 USB input (open, `origin/dane/usb-input-83eb7c`).** No change: USB keys become PS/2 set 1 scancodes in the i8042 ring (`kernel/src/usb.rs:3-4` on the branch), so they pass the same `Decoder`, and `discard_pending` (`kernel/src/terminal.rs:109`) still drops a typed-ahead `y`. When xHCI moves to a ring-3 `usbd` ([ADR 0007](0007-drivers.md)), keys must still reach the terminal through that one decoder.
- **PR #10 (merged).** Its 50-task gate stays as the `script` regression gate; S02-1's action gate runs beside it.

## Open questions for the founder

1. When you plainly ask for something with no undo (stop a program, restart a service, later "empty the trash"), should it still ask once? The roadmap says yes (roadmap.md:130, :140); your words "if i tell it to do something it should just do it" read as no. This ADR asks until you decide.
2. Network: roadmap.md:144 puts network actions in the ask tier. Proposal: opening or fetching a site you named runs; anything that sends your data out, or goes to a site you did not name, asks. "Named" means a host you typed, one from a shipped table of well-known names, or one you said yes to before. The OS's own signed sources (updates, models, packages, the Linux layer's mirrors) count as named (D3). Approve the change?
3. Sending messages: always ask before sending, or send after a short hold (for example 10 s) with `undo`, like Gmail's undo-send?
4. Indirect requests ("make it snappier"): run the AI's reading with "taken as ... undo", or ask first? This ADR asks until you decide.
5. In v0.1 the "did you ask for it" check reads English only, so requests in other languages always ask. Acceptable for v0.1?
6. Text you paste into the prompt counts as your own request. Acceptable?
7. Is running existing MCP tools (from Linux software or other vendors) a product goal for v0.2? It decides how much of the bridge in D9 to build.
8. The action log stores your requests on disk from P2. How long should it keep them, and should there be a `forget` command?
9. Moves and overwrites you plainly ask for would run at once with undo; roadmap.md:141 says they ask. Approve the change?
10. Rhai stays the script language, so its required `smartstring` 1.0.1 dependency (MPL-2.0+; unmaintained per RUSTSEC-2026-0249, 2026-05-03) stays. Rhai 1.27.0, released 2026-10-21, still lists `smartstring ^1.0.0` as required; `compact_str` is only an optional feature (crates.io, read 2026-10-21), so upgrading does not remove it. Is MPL-2.0 acceptable for this one dependency? ADR 0009's question 3 asks the same; one answer covers both.

No permissive crate needs approval here. The Rhai 1.27.0 step brings rhai_codegen 3.3.0 (MIT OR Apache-2.0); ADR 0012's PR lists both. If S02-1 or P5 adds one (rmcp 3.6.0 is Apache-2.0), the PR lists it with its licence.

## Sources

Repo, main at ec5ecff: `docs/roadmap.md:84`, `:112`, `:127-130`, `:140-147`, `:187`; `docs/decisions.md:82`, `:148`, `:171`, `:174`, `:185`, `:203`, `:205`, `:211`; `llmproto/src/script.rs:19-23`, `:36-39`, `:51-94`, `:349-371`; `llmproto/src/local.rs:11-12`, `:26-50`; `infer/src/json.rs:20-25`, `:64`; `infer/src/chat.rs:325-375`, `:402`; `shellcore/src/tiers.rs:16`, `:35-67`; `shellcore/src/undo.rs:13-55`; `shellcore/src/actions.rs:31-34`, `:52-96`, `:111-139`, `:221-243`; `shellcore/src/script.rs:51-57`, `:173-211`; `shellcore/src/commands.rs:107-115`; `shellcore/src/tune.rs:39`, `:68`; `shellcore/src/log.rs:9`; `shellcore/src/lib.rs:78`; `shellcore/Cargo.toml:12`; `kabi/src/lib.rs:82-108`, `:160`; `kabi/src/model.rs:65`; `kabi/src/live.rs:211`; `kernel/src/llm.rs:74-80`; `kernel/src/sched_control/calls.rs:16-35`; `kernel/src/sched_control.rs:30`; `kernel/src/edit_mode.rs:187`; `kernel/src/terminal.rs:87`, `:109`, `:180`, `:235-239`; `kernel/src/console/status.rs:21`, `:41-47`; `user/src/bin/shell.rs:142-147`; `xtask/src/qemu.rs:353-357`; `xtask/src/fluency/judge.rs:61-65`; `termcore/src/style.rs:18`; `testkit/hostile.rs`. Branches: `origin/dane/usb-input-83eb7c` `kernel/src/usb.rs:3-4`; `origin/dane/net-dhcp-dns-b5bdb7`; `origin/dane/virtio-gpu-a5daaa`. GitHub Actions run 39675848260 (2026-10-21, 21:05-21:27 UTC, ubuntu-24.04, 4 vCPU; per-task times from its log).

External, read 2026-10-21 unless dated otherwise:

- MCP specification 2026-07-28, tools and schema: https://modelcontextprotocol.io/specification/2026-07-28/server/tools, https://modelcontextprotocol.io/specification/2026-07-28/schema; licence: https://github.com/modelcontextprotocol/modelcontextprotocol (LICENSE).
- Qwen3-4B-Instruct-2507, Apache-2.0, `tokenizer_config.json` chat template: https://huggingface.co/Qwen/Qwen3-4B-Instruct-2507
- Anthropic, Claude Code auto mode, 2026-03-25: https://www.anthropic.com/engineering/claude-code-auto-mode; permission rules: https://code.claude.com/docs/en/permissions
- CaMeL, arXiv 2503.18813 v2 (2025-06-24): https://arxiv.org/abs/2503.18813; https://github.com/google-research/camel-prompt-injection (Apache-2.0)
- FIDES, arXiv 2505.23643 v2 (2025-09-03): https://arxiv.org/abs/2505.23643; https://github.com/microsoft/fides (MIT)
- AgentDojo 0.1.35 (2025-10-27), arXiv 2406.13352 v3: https://github.com/ethz-spylab/agentdojo (MIT)
- The Attacker Moves Second, arXiv 2510.09023 v1 (2025-10-10): https://arxiv.org/abs/2510.09023
- BFCL: https://github.com/ShishirPatil/gorilla (Apache-2.0)
- SGLang compressed FSM and jump-forward, 2024-02-05: https://lmsys.org/blog/2024-02-05-compressed-fsm/
- Saga pattern, 2025-02-25: https://learn.microsoft.com/en-us/azure/architecture/patterns/saga; compensating transaction, 2026-04-16: https://learn.microsoft.com/en-us/azure/architecture/patterns/compensating-transaction
- Transactional outbox: https://microservices.io/patterns/data/transactional-outbox.html; Gmail undo send: https://support.google.com/mail/answer/2819488; Emacs undo: https://www.gnu.org/software/emacs/manual/html_node/emacs/Undo.html
- Rhai 1.27.0 (2026-10-21T13:05Z, MIT OR Apache-2.0): https://crates.io/crates/rhai/1.27.0; source at tag v1.27.0, `Cargo.toml:68`, `src/ast/ast.rs:780-787`, `src/packages/string_more.rs:24-29`, `src/types/dynamic.rs:563`: https://github.com/rhaiscript/rhai/tree/v1.27.0; dependencies: https://crates.io/api/v1/crates/rhai/1.27.0/dependencies; rhai_codegen 3.3.0 (2026-10-21T13:04Z, MIT OR Apache-2.0): https://crates.io/crates/rhai_codegen/3.3.0; Rhai features: https://rhai.rs/book/start/features.html
- smartstring 1.0.1 (2022-03-24, MPL-2.0+): https://crates.io/crates/smartstring; RUSTSEC-2026-0249 (2026-05-03): https://rustsec.org/advisories/RUSTSEC-2026-0249.html
- rmcp 3.6.0 (2026-10-20, Apache-2.0): https://crates.io/crates/rmcp
- llguidance 1.9.1 (2026-09-30, MIT): https://crates.io/crates/llguidance, https://github.com/guidance-ai/llguidance; outlines-core 0.2.14 (2026-01-09, Apache-2.0): https://crates.io/crates/outlines-core
- GoEX last commit 2024-07-03: https://github.com/ShishirPatil/gorilla/tree/main/goex
