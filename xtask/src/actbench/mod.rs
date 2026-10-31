//! `cargo xtask actbench --local ID [--store DIR] [--arm ARM,..] [--seed N,..]`
//!
//! Spikes S02-1 (the action gate, ADR 0002) and S10-1 (pack format and scale,
//! ADR 0010) as one harness, on the host engine with a catalog model, run on
//! GitHub's runners only (`actbench-model.yml`), never on the founder's PC.
//! Each arm answers 190 tasks ([`tasks`]) and the M8 50 through `script`:
//!
//! - `dsl-1`, `hermes-1`, `prose-1`: the pack in that format over the 40-row
//!   mock registry ([`registry`]), answered with one tagged call under the
//!   prototype decoder ([`grammar`]) with jump-forward. Add `@80` for the
//!   80-row scale and `:no-think` for the `/no_think` tag on each user turn.
//! - `m8`, `m8-a0`: today's prompt and envelope, with and without `/no_think`
//!   (arm A0), on the M8 50 only.
//! - `act`: what ships, task 18's gate. The 190 tasks as `dsl-1+m8script`; the
//!   M8 50 through the shell's one door (`shellcore`'s `act` then script
//!   fallback), answered by llm's own `llmproto::local::act::routed` on the OS
//!   pack. With `--gate` a missed bar fails the run.
//! - `os`: the OS rows' own requests ([`os::TASKS`], such as "stop build-3" and
//!   "restart the indexer") through llm's `act` path on the OS pack and the one
//!   door's name resolution, each against what must run; it passes at 9 of 10.
//! - `view`: view-then-act ([`view::TASKS`], ADR 0005 §6) in a desktop of
//!   S05-2's mock windows: `ui_view`, then `ui_act` on the right control after
//!   reading the view as `Data`. Reported, not gated yet.
//!
//! Code decides what each call would do ([`consent`]), so false runs and asks
//! are counted, and [`score`] holds S02-1's thresholds. The layout is ADR
//! 0010's: PACK, then the `act` MODE turn, history turns, and the request with
//! its State line. The engine runs at 4,096 positions, so every arm fits, on
//! all the runner's CPUs (the same bits as one). Every run is written to
//! `target/actbench/<unix time>-<arm>-s<seed>.jsonl`, headed by `pack_id`.

mod apps;
mod consent;
mod grammar;
mod m8;
mod os;
mod registry;
mod render;
mod score;
mod tasks;
mod view;

use crate::fluency::host::GateHost;
use crate::fluency::local::Shared;
use crate::fluency::tasks::TASKS as M8_TASKS;
use grammar::Tagged;
use infer::Next;
use infer::Sampler;
use infer::chat::{Budget, Ending, Engine, Role, Turn};
use quant::{Parallel, Tier};
use render::Format;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tasks::{Cat, TASKS};

/// ADR 0010 S10-1: the host engine runs at 4,096 positions.
const CONTEXT: usize = 4096;
/// The answer reserve (ADR 0002 D4).
const MAX_TOKENS: usize = 1024;
const SEEDS: [u64; 3] = [42, 1, 2];

/// Workers on std threads, started for each broadcast.
struct Threads(usize);

impl Parallel for Threads {
    fn workers(&self) -> usize {
        self.0
    }

    fn broadcast(&self, job: &(dyn Fn(usize, usize) + Sync)) {
        std::thread::scope(|s| {
            for worker in 1..self.0 {
                s.spawn(move || job(worker, self.0));
            }
            job(0, self.0);
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Tagged(Format),
    /// Today's script path; `no_think` false is arm A0.
    M8,
    /// The OS rows' calls on llm's own `act` path ([`os`]).
    Os,
    /// View-then-act on llm's own `act` path ([`view`]).
    View,
    /// The shipped apps' rows on llm's own `act` path ([`os::APP_TASKS`]).
    Apps,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Arm {
    kind: Kind,
    rows: usize,
    no_think: bool,
    /// `+rules`: the M8 prompt's Rhai rules as the card.
    rules: bool,
    /// `+m8script`: a `script` call's code comes from today's script prompt
    /// and envelope (the plan's fallback for M8-style requests).
    m8script: bool,
    /// `act`: what ships. The 190 tasks as `dsl-1+m8script` with the shipping
    /// decoder and consent; the M8 50 through the shell's one door, with llm's
    /// own `act` path (`llmproto::local::act::routed`) on the OS pack.
    ship: bool,
    /// `+index`: rows no task needs (the 80-row scale's distractors) are
    /// named on index lines only, and close with no arguments (ADR 0010 §6).
    index: bool,
}

impl Arm {
    fn parse(text: &str) -> Result<Self, String> {
        match text {
            "m8" => {
                return Ok(Self {
                    kind: Kind::M8,
                    rows: 40,
                    no_think: true,
                    rules: false,
                    m8script: false,
                    ship: false,
                    index: false,
                });
            }
            "act" => {
                return Ok(Self {
                    kind: Kind::Tagged(Format::Dsl1),
                    rows: 40,
                    no_think: false,
                    rules: false,
                    m8script: true,
                    ship: true,
                    index: false,
                });
            }
            "os" => {
                return Ok(Self {
                    kind: Kind::Os,
                    rows: 0,
                    no_think: false,
                    rules: false,
                    m8script: false,
                    ship: true,
                    index: false,
                });
            }
            "apps" => {
                return Ok(Self {
                    kind: Kind::Apps,
                    rows: 0,
                    no_think: false,
                    rules: false,
                    m8script: false,
                    ship: true,
                    index: false,
                });
            }
            "view" => {
                return Ok(Self {
                    kind: Kind::View,
                    rows: 0,
                    no_think: false,
                    rules: false,
                    m8script: false,
                    ship: true,
                    index: false,
                });
            }
            "m8-a0" => {
                return Ok(Self {
                    kind: Kind::M8,
                    rows: 40,
                    no_think: false,
                    rules: false,
                    m8script: false,
                    ship: false,
                    index: false,
                });
            }
            _ => {}
        }
        let (text, rows) = match text.split_once('@') {
            Some((arm, "80")) => (arm, 80),
            Some((arm, "40")) => (arm, 40),
            Some(_) => return Err(format!("{text}: the scale is @40 or @80")),
            None => (text, 40),
        };
        let (text, no_think) = match text.strip_suffix(":no-think") {
            Some(arm) => (arm, true),
            None => (text, false),
        };
        let (mut text, mut rules, mut m8script, mut index) = (text, false, false, false);
        loop {
            if let Some(arm) = text.strip_suffix("+rules") {
                (text, rules) = (arm, true);
            } else if let Some(arm) = text.strip_suffix("+m8script") {
                (text, m8script) = (arm, true);
            } else if let Some(arm) = text.strip_suffix("+index") {
                (text, index) = (arm, true);
            } else {
                break;
            }
        }
        let format = Format::parse(text).ok_or(format!(
            "unknown arm {text}: dsl-1, hermes-1, prose-1, m8 or m8-a0"
        ))?;
        Ok(Self {
            kind: Kind::Tagged(format),
            rows,
            no_think,
            rules,
            m8script,
            ship: false,
            index,
        })
    }

    fn label(&self) -> String {
        match self.kind {
            Kind::M8 if self.no_think => "m8".into(),
            Kind::M8 => "m8-a0".into(),
            Kind::Os => "os".into(),
            Kind::View => "view".into(),
            Kind::Apps => "apps".into(),
            Kind::Tagged(_) if self.ship => "act".into(),
            Kind::Tagged(format) => format!(
                "{}{}{}{}{}@{}",
                format.name(),
                if self.rules { "+rules" } else { "" },
                if self.m8script { "+m8script" } else { "" },
                if self.index { "+index" } else { "" },
                if self.no_think { ":no-think" } else { "" },
                self.rows
            ),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    id: String,
    store: Option<String>,
    arms: Vec<Arm>,
    seeds: Vec<u64>,
    /// Run only the first this many tasks of each category (and of the M8 50).
    limit: Option<usize>,
    m8: bool,
    /// Only the M8 tasks, for an arm that changes only the script path.
    only_m8: bool,
    /// Task 18's gate: fail when any arm at any seed misses a bar.
    gate: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let (mut id, mut store, mut limit) = (None, None, None);
    let mut arms = vec![Arm::parse("dsl-1")?];
    let mut seeds = SEEDS.to_vec();
    let (mut m8, mut only_m8, mut gate) = (true, false, false);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = |what: &str| rest.next().cloned().ok_or(format!("{arg} expects {what}"));
        match arg.as_str() {
            "--local" => id = Some(value("a catalog model id")?),
            "--store" => store = Some(value("a directory")?),
            "--arm" => {
                arms = value("arms, comma-separated")?
                    .split(',')
                    .map(Arm::parse)
                    .collect::<Result<_, _>>()?;
            }
            "--seed" => {
                seeds = value("seeds, comma-separated")?
                    .split(',')
                    .map(|s| s.parse().map_err(|_| format!("bad seed {s}")))
                    .collect::<Result<_, _>>()?;
            }
            "--limit" => {
                limit = Some(
                    value("a count")?
                        .parse()
                        .map_err(|_| "--limit expects a count".to_string())?,
                );
            }
            "--no-m8" => m8 = false,
            "--only-m8" => only_m8 = true,
            "--gate" => gate = true,
            other => return Err(format!("unknown actbench option {other}")),
        }
    }
    Ok(Options {
        id: id.ok_or("actbench needs --local ID")?,
        store,
        arms,
        seeds,
        limit,
        m8,
        only_m8,
        gate,
    })
}

/// The engine and what the report says about its model.
pub(crate) struct Loaded {
    engine: Shared,
    tokenizer: bpe::Tokenizer,
    tokenizer_sha256: String,
    line: String,
}

/// The engine for model `bytes` at [`CONTEXT`] positions on every CPU.
fn load(bytes: &'static [u8], name: &str) -> Result<Loaded, String> {
    let started = Instant::now();
    let budget = Budget {
        kv_bytes: 1 << 30,
        other_bytes: kabi::model::ENGINE_OTHER_BYTES as usize * 2,
    };
    let mut engine =
        Engine::load(bytes, Tier::best(), CONTEXT, budget).map_err(|e| e.to_string())?;
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads: &'static Threads = Box::leak(Box::new(Threads(workers)));
    engine.set_parallel(threads);
    let file = gguf::Gguf::parse(bytes).map_err(|e| format!("{e:?}"))?;
    let tokenizer = bpe::Tokenizer::from_gguf(file.header()).map_err(|e| format!("{e:?}"))?;
    let mut all = Vec::new();
    for id in 0..tokenizer.len() as u32 {
        let token = tokenizer.token_bytes(id);
        all.extend_from_slice(&(token.len() as u32).to_le_bytes());
        all.extend_from_slice(token);
    }
    let about = engine.about();
    let line = format!(
        "{} ({} tier, context {}, {} workers), loaded in {} ms",
        about.name.as_deref().unwrap_or(name),
        about.tier.name(),
        about.context,
        about.workers,
        started.elapsed().as_millis(),
    );
    Ok(Loaded {
        engine: Rc::new(RefCell::new(engine)),
        tokenizer,
        tokenizer_sha256: crate::sha256_hex(&all),
        line,
    })
}

/// PACK, then the `act` MODE turn.
fn base_turns(pack: &str) -> Vec<(Role, String)> {
    vec![
        (Role::System, pack.to_string()),
        (Role::System, render::MODE_ACT.to_string()),
    ]
}

/// The current turn: the State line, if any, and the request.
fn user_text(state: &str, line: &str, no_think: bool) -> String {
    let state = match state {
        "" => String::new(),
        state => format!("State: {state}\n"),
    };
    let tag = if no_think { "\n/no_think" } else { "" };
    format!("{state}Request: {line}{tag}")
}

/// A task's conversation: history turns as code wrote them, then its own.
fn task_turns(pack: &str, task: &tasks::Task, no_think: bool) -> Vec<(Role, String)> {
    let mut turns = base_turns(pack);
    for turn in task.history {
        turns.push((Role::User, format!("Request: {}", turn.line)));
        turns.push((Role::Assistant, turn.call.to_string()));
        turns.push((Role::User, format!("Result: {}", turn.result)));
    }
    turns.push((Role::User, user_text(task.state, task.line, no_think)));
    turns
}

/// One decoded answer.
struct Decoded {
    text: String,
    call: Option<Value>,
    tokens: usize,
    forced: usize,
    ending: Ending,
    prompt_tokens: usize,
    cached: usize,
}

/// The tagged call for `turns`, drawn as scripts are (temperature 0.2) with
/// `seed`.
fn decode(
    engine: &mut Engine<'_>,
    grammar: &Tagged,
    turns: &[(Role, String)],
    seed: u64,
) -> Result<Decoded, String> {
    let turns: Vec<Turn<'_>> = turns.iter().map(|(r, t)| Turn::new(*r, t)).collect();
    let mut sampler = Sampler::seeded(llmproto::script::TEMPERATURE, seed);
    let reply = engine
        .reply_under(
            &turns,
            MAX_TOKENS,
            &mut sampler,
            Some(grammar),
            true,
            |_| Next::Continue,
        )
        .map_err(|e| llmproto::local::describe(&e))?;
    let text = reply.text();
    let call = (reply.ending == Ending::EndToken)
        .then(|| serde_json::from_str(&text).ok())
        .flatten();
    Ok(Decoded {
        text,
        call,
        tokens: reply.tokens,
        forced: reply.forced,
        ending: reply.ending,
        prompt_tokens: reply.prompt_tokens,
        cached: reply.cached,
    })
}

/// The tasks a run takes: all, or the first `limit` of each category.
fn chosen(limit: Option<usize>) -> Vec<&'static tasks::Task> {
    let Some(limit) = limit else {
        return TASKS.iter().collect();
    };
    Cat::ALL
        .iter()
        .flat_map(|&cat| TASKS.iter().filter(move |t| t.cat == cat).take(limit))
        .collect()
}

/// The M8 50 (or the first `limit`) through `transport()`, as (passed, runs).
fn run_m8(
    limit: Option<usize>,
    host: &dyn Fn() -> GateHost,
    records: &mut Vec<Value>,
) -> (usize, usize) {
    let mut passed = 0;
    let count = limit.unwrap_or(M8_TASKS.len()).min(M8_TASKS.len());
    for (number, task) in M8_TASKS.iter().take(count).enumerate() {
        let clock = Instant::now();
        let outcome = crate::fluency::attempt_on(task, host());
        let ms = clock.elapsed().as_millis();
        passed += usize::from(outcome.failure.is_none());
        let failure = outcome.failure.as_ref().map(|(k, d)| format!("{k:?}: {d}"));
        println!(
            "  {} m8 {:>2} {} ({ms} ms)",
            if outcome.failure.is_none() {
                "ok  "
            } else {
                "FAIL"
            },
            number + 1,
            task.request
        );
        records.push(json!({
            "cat": "m8", "task": number + 1, "line": task.request, "rounds": outcome.rounds,
            "passed": outcome.failure.is_none(), "failure": failure, "script": outcome.script,
            "ms": ms,
        }));
    }
    (passed, count)
}

/// One arm at one seed: its records, and whether every gate passed.
fn run_arm(
    loaded: &Loaded,
    arm: Arm,
    seed: u64,
    options: &Options,
    target_dir: &Path,
) -> Result<(bool, String), String> {
    match arm.kind {
        Kind::Os => return os::run(loaded, seed, target_dir),
        Kind::View => return view::run(loaded, seed, target_dir),
        Kind::Apps => return os::run_apps(loaded, seed, target_dir),
        _ => {}
    }
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let rows = registry::rows(arm.rows);
    let mut header = json!({
        "arm": arm.label(), "seed": seed, "model": loaded.line,
        "commit": std::env::var("GITHUB_SHA").ok(),
        "run_id": std::env::var("GITHUB_RUN_ID").ok(),
        "runner": std::env::var("RUNNER_NAME").ok(),
        "context": CONTEXT, "started_unix": started,
        "tasks_sha256": crate::sha256_hex(include_str!("tasks.rs").replace("\r\n", "\n").as_bytes()),
    });
    let mut records = Vec::new();
    let mut scored = Vec::new();
    let m8 = match arm.kind {
        Kind::Os | Kind::View | Kind::Apps => unreachable!("arms os, view and apps returned above"),
        Kind::M8 => {
            header["pack_id"] = json!(null);
            let engine = loaded.engine.clone();
            let no_think = arm.no_think;
            Some(run_m8(
                options.limit,
                &move || GateHost::new(m8::reference(engine.clone(), no_think)),
                &mut records,
            ))
        }
        Kind::Tagged(format) => {
            let pack = match arm.index {
                true => render::indexed_card(format, &rows, arm.rules),
                false => render::pack_card(format, &rows, arm.rules),
            };
            let mut ids = Vec::new();
            loaded.tokenizer.encode(&pack, &mut ids);
            header["format"] = json!(format.name());
            header["rows"] = json!(arm.rows);
            header["pack_id"] = json!(render::pack_id(format, &loaded.tokenizer_sha256, &pack));
            header["pack_tokens"] = json!(ids.len());
            header["pack_bytes"] = json!(pack.len());
            println!(
                "xtask: actbench: {} seed {seed}, pack {} tokens, pack_id {}",
                arm.label(),
                ids.len(),
                header["pack_id"]
            );
            let grammar = Rc::new(match arm.index {
                true => grammar::indexed(&rows),
                false => grammar::tagged(&rows),
            });
            let clock = Instant::now();
            let base = base_turns(&pack);
            let warm: Vec<Turn<'_>> = base.iter().map(|(r, t)| Turn::new(*r, t)).collect();
            let _ =
                loaded
                    .engine
                    .borrow_mut()
                    .reply(&warm, 1, &mut Sampler::greedy(), None, |_| Next::Continue);
            header["cold_prefill_ms"] = json!(clock.elapsed().as_millis());
            let tasks = if options.only_m8 {
                Vec::new()
            } else {
                chosen(options.limit)
            };
            for task in tasks {
                let clock = Instant::now();
                let turns = task_turns(&pack, task, arm.no_think);
                // An engine error fails this task, not the run.
                let decoded = decode(&mut loaded.engine.borrow_mut(), &grammar, &turns, seed)
                    .unwrap_or_else(|error| Decoded {
                        text: format!("error: {error}"),
                        call: None,
                        tokens: 0,
                        forced: 0,
                        ending: Ending::Stopped,
                        prompt_tokens: 0,
                        cached: 0,
                    });
                let ms = clock.elapsed().as_millis();
                let decision = decoded.call.as_ref().map_or(consent::Decision::Never, |c| {
                    consent::decide(&rows, c, task.line, task.state)
                });
                let free = decoded.tokens - decoded.forced;
                let one = score::score(&rows, task, decoded.call.as_ref(), decision, free);
                println!(
                    "  {} {:<10} {} -> {} [{}] ({ms} ms, {free} free + {} forced)",
                    if one.passed { "ok  " } else { "FAIL" },
                    task.cat.name(),
                    task.line,
                    decoded.text,
                    decision.name(),
                    decoded.forced,
                );
                records.push(json!({
                    "cat": task.cat.name(), "line": task.line, "state": task.state,
                    "answer": decoded.text, "ending": format!("{:?}", decoded.ending),
                    "decision": decision.name(), "passed": one.passed,
                    "false_run": one.false_run, "false_ask": one.false_ask,
                    "tokens": decoded.tokens, "forced": decoded.forced,
                    "prompt_tokens": decoded.prompt_tokens, "cached": decoded.cached, "ms": ms,
                }));
                scored.push(one);
            }
            if arm.ship {
                // What the OS sends: llmproto's pack, not the mock one.
                let os_pack = llmproto::pack::render(llmproto::pack::Format::Dsl1);
                let mut os_ids = Vec::new();
                loaded.tokenizer.encode(&os_pack, &mut os_ids);
                let id = llmproto::pack::pack_id(
                    llmproto::pack::Format::Dsl1,
                    &loaded.tokenizer_sha256,
                    &os_pack,
                );
                header["os_pack_id"] = json!(format!("{id:016x}"));
                header["os_pack_tokens"] = json!(os_ids.len());
            }
            options.m8.then(|| {
                if arm.ship {
                    let engine = loaded.engine.clone();
                    // What ships: the OS rows and the shipped apps' turn.
                    let grammar = Rc::new(apps::grammar().expect("the shipped rows decode"));
                    return run_m8(
                        options.limit,
                        &move || m8::shipping(engine.clone(), grammar.clone()),
                        &mut records,
                    );
                }
                // The gate's shell has only the M8 functions, so the pack says so.
                let m8_rows = registry::m8_rows();
                let tagging = Rc::new(m8::Tagging {
                    engine: loaded.engine.clone(),
                    grammar: Rc::new(grammar::tagged(&m8_rows)),
                    pack: Rc::new(render::pack_card(format, &m8_rows, arm.rules)),
                    m8script: arm.m8script,
                    seed,
                    no_think: arm.no_think,
                });
                run_m8(
                    options.limit,
                    &move || GateHost::new(m8::tagged(tagging.clone())),
                    &mut records,
                )
            })
        }
    };
    let gates = match arm.kind {
        Kind::Os | Kind::View | Kind::Apps => unreachable!("arms os, view and apps returned above"),
        Kind::M8 => score::gates(&[], m8).split_off(7),
        Kind::Tagged(_) if options.only_m8 => score::gates(&[], m8).split_off(7),
        Kind::Tagged(_) => score::gates(&scored, m8),
    };
    let categories: serde_json::Map<String, Value> = Cat::ALL
        .iter()
        .map(|&cat| {
            let (passed, runs) = score::count(&scored, cat);
            (cat.name().to_string(), json!([passed, runs]))
        })
        .collect();
    let mut summary = json!({"categories": categories, "m8": m8.map(|(p, r)| json!([p, r]))});
    println!("xtask: actbench {} seed {seed}:", arm.label());
    for (cat, value) in &categories {
        println!("  {cat:<10} {}/{}", value[0], value[1]);
    }
    for gate in &gates {
        println!(
            "  {} {:<36} {:>7}  (pass {})",
            if gate.passes { "pass" } else { "FAIL" },
            gate.name,
            gate.value,
            gate.bar
        );
        summary[gate.name] = json!({"value": gate.value, "bar": gate.bar, "passes": gate.passes});
    }
    let passes = gates.iter().all(|g| g.passes);
    summary["meets_gate"] = json!(passes);
    let dir = target_dir.join("actbench");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = arm.label().replace([':', '@'], "_");
    let path = dir.join(format!("{started}-{name}-s{seed}.jsonl"));
    let lines: Vec<String> = std::iter::once(&header)
        .chain(&records)
        .chain(std::iter::once(&summary))
        .map(Value::to_string)
        .collect();
    fs::write(&path, lines.join("\n") + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    println!("xtask: runs written to {}", path.display());
    Ok((passes, summary_rows(&arm, seed, &header, &gates)))
}

/// One arm at one seed as Markdown: its pack ids, then each bar.
fn summary_rows(arm: &Arm, seed: u64, header: &Value, gates: &[score::Gate]) -> String {
    let id = |key: &str| {
        header[key]
            .as_str()
            .map_or(String::from("-"), |id| format!("`{id}`"))
    };
    let mut out = format!(
        "\n### {} seed {seed}\n\npack_id {} ({} tokens), OS pack_id {} ({} tokens)\n\n\
         | Bar | Value | Pass |\n|---|---|---|\n",
        arm.label(),
        id("pack_id"),
        header["pack_tokens"],
        id("os_pack_id"),
        header["os_pack_tokens"],
    );
    for gate in gates {
        let mark = if gate.passes { "yes" } else { "**no**" };
        let row = format!(
            "| {} ({}) | {} | {mark} |\n",
            gate.name, gate.bar, gate.value
        );
        out.push_str(&row);
    }
    out
}

/// `cargo xtask actbench`: every arm at every seed. It writes
/// `target/actbench/summary.md` (and the job summary on GitHub). With `--gate`
/// (task 18) it fails when any arm at any seed misses a bar; without, it
/// reports and exits 0, as S02-1's measurements did.
pub fn main(args: &[String], target_dir: &Path) -> Result<(), String> {
    if let Some(("--rescore", files)) = args.split_first().map(|(a, r)| (a.as_str(), r)) {
        return files.iter().try_for_each(|file| rescore(Path::new(file)));
    }
    let options = parse(args)?;
    let path = crate::models::stored(&options.id, options.store.as_deref())?;
    let bytes = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let loaded = load(bytes, &options.id)?;
    println!("xtask: actbench: {}", loaded.line);
    let mut results = Vec::new();
    let mut summary = format!(
        "## actbench\n\n{} on {}, commit {}\n",
        loaded.line,
        std::env::var("RUNNER_NAME").unwrap_or_else(|_| String::from("this machine")),
        std::env::var("GITHUB_SHA").unwrap_or_else(|_| String::from("(local)")),
    );
    let mut failed = Vec::new();
    for &arm in &options.arms {
        for &seed in &options.seeds {
            let (passes, rows) = run_arm(&loaded, arm, seed, &options, target_dir)?;
            summary.push_str(&rows);
            if !passes {
                failed.push(format!("{} seed {seed}", arm.label()));
            }
            results.push(format!(
                "{} seed {seed}: {}",
                arm.label(),
                if passes { "passes" } else { "fails" }
            ));
        }
    }
    for line in results {
        println!("xtask: actbench {line}");
    }
    write_summary(&summary, target_dir)?;
    match (options.gate, failed.is_empty()) {
        (true, false) => Err(format!("the actbench gate fails: {}", failed.join(", "))),
        _ => Ok(()),
    }
}

/// The run's Markdown, to `target/actbench/summary.md` and, on GitHub, the
/// job's summary.
fn write_summary(summary: &str, target_dir: &Path) -> Result<(), String> {
    let path = target_dir.join("actbench").join("summary.md");
    fs::create_dir_all(target_dir.join("actbench")).map_err(|e| e.to_string())?;
    fs::write(&path, summary).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(step) = std::env::var("GITHUB_STEP_SUMMARY")
        .ok()
        .filter(|s| !s.is_empty())
    {
        use std::io::Write as _;
        fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&step)
            .and_then(|mut file| file.write_all(summary.as_bytes()))
            .map_err(|e| format!("{step}: {e}"))?;
    }
    Ok(())
}

/// `cargo xtask actbench --rescore FILE..`: a tagged arm's recorded answers
/// judged again with today's consent and scoring, no model needed. The M8 tasks
/// keep their recorded result.
fn rescore(path: &Path) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let lines: Vec<Value> = text
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let header = lines.first().ok_or("an empty run")?;
    let rows = registry::rows(header["rows"].as_u64().unwrap_or(40) as usize);
    let mut scored = Vec::new();
    let (mut m8, mut m8_runs) = (0, 0);
    for record in &lines[1..] {
        match record["cat"].as_str() {
            Some("m8") => {
                m8_runs += 1;
                m8 += usize::from(record["passed"] == true);
            }
            Some(cat) => {
                let line = record["line"].as_str().unwrap_or("");
                let Some(task) = TASKS.iter().find(|t| t.cat.name() == cat && t.line == line)
                else {
                    continue;
                };
                let answer = record["answer"].as_str().unwrap_or("");
                let call: Option<Value> = serde_json::from_str(answer).ok();
                let decision = call.as_ref().map_or(consent::Decision::Never, |c| {
                    consent::decide(&rows, c, task.line, task.state)
                });
                let free = (record["tokens"].as_u64().unwrap_or(0)
                    - record["forced"].as_u64().unwrap_or(0)) as usize;
                scored.push(score::score(&rows, task, call.as_ref(), decision, free));
            }
            None => {}
        }
    }
    println!(
        "{} ({} seed {}):",
        path.display(),
        header["arm"],
        header["seed"]
    );
    for gate in score::gates(&scored, (m8_runs > 0).then_some((m8, m8_runs))) {
        let mark = if gate.passes { "pass" } else { "FAIL" };
        println!(
            "  {mark} {:<36} {:>7}  (pass {})",
            gate.name, gate.value, gate.bar
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use infer::json::Constraint;

    fn args(text: &str) -> Vec<String> {
        text.split(' ').map(String::from).collect()
    }

    #[test]
    fn arms_and_seeds_parse() {
        let o = parse(&args(
            "--local q --arm dsl-1,hermes-1@80,dsl-1:no-think,m8-a0 --seed 42",
        ))
        .unwrap();
        assert_eq!(
            o.arms.iter().map(Arm::label).collect::<Vec<_>>(),
            ["dsl-1@40", "hermes-1@80", "dsl-1:no-think@40", "m8-a0"]
        );
        assert_eq!(o.seeds, [42]);
        assert!(parse(&args("--arm dsl-1")).is_err());
        assert!(parse(&args("--local q --arm dsl-2")).is_err());
        assert!(parse(&args("--local q --arm dsl-1@60")).is_err());
        assert_eq!(parse(&args("--local q")).unwrap().seeds, SEEDS);
        let o = parse(&args(
            "--local q --arm dsl-1+rules,dsl-1+m8script@80 --only-m8",
        ))
        .unwrap();
        let labels: Vec<String> = o.arms.iter().map(Arm::label).collect();
        assert_eq!(labels, ["dsl-1+rules@40", "dsl-1+m8script@80"]);
        assert!(o.only_m8 && o.arms[0].rules && o.arms[1].m8script);
    }

    #[test]
    fn the_layout_is_pack_mode_history_then_state_and_request() {
        let task = TASKS.iter().find(|t| !t.history.is_empty()).unwrap();
        let turns = task_turns("PACK", task, false);
        assert_eq!(turns[0], (Role::System, "PACK".into()));
        assert_eq!(turns[1].1, render::MODE_ACT);
        assert_eq!(turns.len(), 2 + 3 * task.history.len() + 1);
        assert_eq!(
            user_text("focused=x", "stop this", true),
            "State: focused=x\nRequest: stop this\n/no_think"
        );
        assert_eq!(chosen(Some(1)).len(), Cat::ALL.len());
    }

    /// The whole loop on the generated local-script model: every task decodes
    /// to a call the grammar closes, the M8 tasks go through the shell, and the
    /// answers are the same at a second run (seeded) and on one worker.
    #[test]
    fn the_harness_runs_end_to_end_on_the_fixture_model() {
        // A vocabulary trained on the pack too, so it fits the fixture's context.
        let text = [
            crate::local_script::prompts(),
            render::pack(Format::Dsl1, &registry::rows(40)),
            render::MODE_ACT.to_string(),
            llmproto::pack::render(llmproto::pack::Format::Dsl1),
        ]
        .join("\n");
        let bytes = infer::fixture::local_script(&text);
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let loaded = load(bytes, "fixture").unwrap();
        let rows = registry::rows(40);
        let grammar = grammar::tagged(&rows);
        let pack = render::pack(Format::Dsl1, &rows);
        let task = &TASKS[0];
        let turns = task_turns(&pack, task, false);
        let first = decode(&mut loaded.engine.borrow_mut(), &grammar, &turns, 42).unwrap();
        let again = decode(&mut loaded.engine.borrow_mut(), &grammar, &turns, 42).unwrap();
        assert_eq!(first.text, again.text);
        assert!(again.cached > 0);
        if first.ending == Ending::EndToken {
            let call = first.call.expect("a closed answer parses");
            assert!(
                grammar.is_done(
                    &grammar
                        .advance(grammar.start(), first.text.as_bytes())
                        .unwrap()
                )
            );
            assert!(call["arguments"].is_object());
        }
        let dir = std::env::temp_dir().join(format!("actbench-test-{}", std::process::id()));
        let options = parse(&args("--local fixture --limit 1 --seed 42")).unwrap();
        run_arm(&loaded, Arm::parse("dsl-1").unwrap(), 42, &options, &dir).unwrap();
        run_arm(&loaded, Arm::parse("m8-a0").unwrap(), 42, &options, &dir).unwrap();
        // What ships: the M8 task through the shell's one door, on the OS pack.
        let (_, rows) = run_arm(&loaded, Arm::parse("act").unwrap(), 42, &options, &dir).unwrap();
        assert!(
            rows.contains("### act seed 42") && rows.contains("OS pack_id `"),
            "{rows}"
        );
        assert!(rows.contains("| exact single calls (>= 57/60) |"), "{rows}");
        let files = fs::read_dir(dir.join("actbench")).unwrap().count();
        assert_eq!(files, 3);
        write_summary(&rows, &dir).unwrap();
        assert!(dir.join("actbench").join("summary.md").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
