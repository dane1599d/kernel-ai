//! Spike S10-2 (ADR 0010 §6, "groups and swap"): do app groups on the index
//! level, with the right group's full lines shown only after the call names
//! it, answer as well as a flat pack that shows every app's lines in full?
//!
//! ```text
//! cargo xtask groups --local ID [--store DIR] [--arm layered,flat,described] [--limit N] [--gate]
//! ```
//!
//! - Arm `layered`: what llm ships. The eight stub apps ([`stubs`]) go in the
//!   apps turn as index lines (`llmproto::pack::apps::turn`); a call naming an
//!   app row stops after the name and is written again with that app's lines
//!   after the request (`llmproto::local::act::act`), at 3,072 positions:
//!   ADR 0010's context. llm itself still loads at `kabi::model::CONTEXT_CAP`
//!   (2,048) today.
//! - Arm `flat`: every app's full lines in the apps turn
//!   (`llmproto::local::act::grammar_flat`), each call written in one round,
//!   at 4,096 positions.
//! - Arm `described`: layered, but each index row in the apps turn carries a
//!   short description from its doc (`calendar_day (the events on a day)`,
//!   `llmproto::local::act::grammar_described`), within the same budget by
//!   the same count: each description is cut to the most words at which
//!   every app fits (`pack::apps::described_turn`), and only if none does
//!   does an app whose described line passes the budget get its plain index
//!   line. Same grammar and second round as layered, at 3,072
//!   positions. Spike only: llm ships layered.
//!
//! Each arm answers the 120 [`tasks`] through llm's own `act` on the shipped
//! pack, with llm's sampler. llm draws with one fixed seed, so a rerun is the
//! same run: there is no seed option. The gate (`--gate`, flat and at least one
//! index arm in one run) checks each index arm (layered, described) against
//! flat: both answer every task with a call (no error and no full context, so
//! a flat arm that never answered cannot make the comparison pass), the index
//! arm scores at least flat less 2 points (of 100), and names the right app on
//! at least 95% of the single-app tasks. Each index arm gets its own verdict;
//! the gate passes when at least one index arm passes, since the question is
//! whether the index level in some form holds up. Every task goes to
//! `target/groups/<arm>.jsonl`, and the figures to the step summary. Real
//! model runs are for GitHub runners only.
//!
//! The segment store's restore-equals-fresh check is infer's own test.

mod stubs;
mod tasks;
#[cfg(test)]
mod tests;

use crate::actbench::{self, Loaded};
use infer::Next;
use llmproto::act::{ActRequest, Call, History};
use llmproto::local::act::{self as local_act, Grammar};
use llmproto::pack::apps::{self, AppGroup};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::time::Instant;
use tasks::{Cat, TASKS, Task, Want};

/// The time llm gives one call. The harness hands it to `act` as llm does;
/// the local engine does not stop on it, so a slow call is only slow.
const TIMEOUT_MS: u64 = 60_000;
/// The gate: layered may trail flat by at most this many points (of 100).
const MAX_TRAIL_POINTS: f64 = 2.0;
/// The gate: the share of single-app tasks whose call names the right app.
const MIN_RIGHT_GROUP: f64 = 0.95;

/// One arm of the spike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arm {
    Layered,
    Flat,
    Described,
}

impl Arm {
    fn parse(text: &str) -> Result<Arm, String> {
        match text {
            "layered" => Ok(Arm::Layered),
            "flat" => Ok(Arm::Flat),
            "described" => Ok(Arm::Described),
            other => Err(format!(
                "unknown groups arm {other} (layered, flat or described)"
            )),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Arm::Layered => "layered",
            Arm::Flat => "flat",
            Arm::Described => "described",
        }
    }

    /// The engine's positions (ADR 0010 S10-2).
    fn context(self) -> usize {
        match self {
            Arm::Layered | Arm::Described => 3072,
            Arm::Flat => 4096,
        }
    }

    fn grammar(self, groups: &[AppGroup]) -> Result<Grammar, String> {
        match self {
            Arm::Layered => local_act::grammar_with(groups),
            Arm::Flat => local_act::grammar_flat(groups),
            Arm::Described => local_act::grammar_described(groups),
        }
        .map_err(|e| e.to_string())
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    id: String,
    store: Option<String>,
    arms: Vec<Arm>,
    /// Only the first this many tasks of each category.
    limit: Option<usize>,
    gate: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let (mut id, mut store, mut limit, mut gate) = (None, None, None, false);
    let mut arms = vec![Arm::Layered, Arm::Flat];
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
            "--limit" => {
                let count = value("a count")?;
                limit = Some(count.parse().map_err(|_| format!("bad --limit {count}"))?);
            }
            "--gate" => gate = true,
            other => return Err(format!("unknown groups option {other}")),
        }
    }
    let indexed = arms.contains(&Arm::Layered) || arms.contains(&Arm::Described);
    if gate && !(indexed && arms.contains(&Arm::Flat)) {
        return Err("--gate compares the arms: run flat with layered or described".into());
    }
    Ok(Options {
        id: id.ok_or("groups needs --local ID")?,
        store,
        arms,
        limit,
        gate,
    })
}

/// The tasks a run answers: all, or the first `limit` of each category.
fn chosen(limit: Option<usize>) -> Vec<&'static Task> {
    let mut seen = [0usize; Cat::ALL.len()];
    TASKS
        .iter()
        .filter(|task| {
            let cat = Cat::ALL.iter().position(|c| *c == task.cat).unwrap_or(0);
            seen[cat] += 1;
            limit.is_none_or(|limit| seen[cat] <= limit)
        })
        .collect()
}

/// Whether `value` is what `want` asks for.
fn fits(want: Want, value: Option<&Value>) -> bool {
    match (want, value) {
        (Want::Text(text), Some(Value::String(got))) => got.to_lowercase().contains(text),
        (Want::Int(n), Some(got)) => got.as_u64() == Some(n),
        _ => false,
    }
}

/// Whether `call` passes `task`: its row with every expected argument, or,
/// for a task for no app, any call that names no app row.
fn passed(task: &Task, groups: &[AppGroup], call: &Call) -> bool {
    match task.row {
        Some(row) => {
            call.name == row
                && task
                    .args
                    .iter()
                    .all(|(arg, want)| fits(*want, call.arguments.get(*arg)))
        }
        None => apps::group_of(groups, &call.name).is_none(),
    }
}

/// For a task for one app: whether `call` names a row of that app. The
/// layered arm's second round is under that row's grammar alone, so the name
/// the call ends with is the one the first round stopped after.
fn right_group(task: &Task, groups: &[AppGroup], call: Option<&Call>) -> Option<bool> {
    let want = apps::group_of(groups, task.row?)?;
    let got = call.and_then(|call| apps::group_of(groups, &call.name));
    Some(got.is_some_and(|got| got.app == want.app))
}

/// What one arm scored.
#[derive(Debug, Default, PartialEq)]
struct Tally {
    /// (passed, asked) for each category.
    cats: [(usize, usize); Cat::ALL.len()],
    /// (right app, asked) over the single-app tasks.
    right: (usize, usize),
    /// Tasks whose `act` gave an error instead of a call.
    errors: usize,
    /// Of those, the ones whose request filled the model's context.
    context_full: usize,
    prompt_positions: usize,
    millis: Vec<u128>,
}

impl Tally {
    fn passed(&self) -> (usize, usize) {
        self.cats
            .iter()
            .fold((0, 0), |(p, a), c| (p + c.0, a + c.1))
    }

    /// Passed tasks as points of 100.
    fn points(&self) -> f64 {
        let (passed, asked) = self.passed();
        100.0 * passed as f64 / asked.max(1) as f64
    }

    fn right_share(&self) -> f64 {
        self.right.0 as f64 / self.right.1.max(1) as f64
    }

    fn median_ms(&self) -> u128 {
        let mut sorted = self.millis.clone();
        sorted.sort_unstable();
        sorted.get(sorted.len() / 2).copied().unwrap_or(0)
    }
}

/// `task` asked of `engine` under `grammar`, as llm's `act` answers.
fn ask(loaded: &Loaded, grammar: &Grammar, task: &Task) -> Result<Call, String> {
    let request = ActRequest {
        line: task.line.into(),
        state: String::new(),
        timeout_ms: TIMEOUT_MS,
        read: None,
        apps: String::new(),
    };
    local_act::act(
        &mut loaded.engine.borrow_mut(),
        grammar,
        &request,
        &History::new(),
        |_| Next::Continue,
    )
    .map_err(|error| error.describe())
}

/// One arm over `tasks`: its tally, with each task written to
/// `target/groups/<arm>.jsonl`.
fn run_arm(
    loaded: &Loaded,
    arm: Arm,
    groups: &[AppGroup],
    tasks: &[&Task],
    target_dir: &Path,
) -> Result<Tally, String> {
    let grammar = arm.grammar(groups)?;
    let mut tally = Tally {
        prompt_positions: local_act::pack_tokens(&loaded.engine.borrow(), &grammar),
        ..Tally::default()
    };
    let mut records = Vec::new();
    for task in tasks {
        let started = Instant::now();
        let call = ask(loaded, &grammar, task);
        let millis = started.elapsed().as_millis();
        tally.millis.push(millis);
        let ok = call.as_ref().is_ok_and(|call| passed(task, groups, call));
        let cat = Cat::ALL.iter().position(|c| *c == task.cat).unwrap_or(0);
        tally.cats[cat].0 += usize::from(ok);
        tally.cats[cat].1 += 1;
        let right = right_group(task, groups, call.as_ref().ok());
        if task.cat == Cat::Single
            && let Some(right) = right
        {
            tally.right.0 += usize::from(right);
            tally.right.1 += 1;
        }
        let got = match &call {
            Ok(call) => call.text(),
            Err(why) => {
                tally.errors += 1;
                tally.context_full += usize::from(why.contains("filled the model's context"));
                why.clone()
            }
        };
        if !ok {
            println!(
                "  FAIL {} {:<10} {} -> {got}",
                arm.name(),
                task.cat.name(),
                task.line
            );
        }
        records.push(json!({
            "arm": arm.name(), "cat": task.cat.name(), "line": task.line,
            "want": task.row, "got": got, "passed": ok, "right_app": right, "ms": millis,
        }));
    }
    let dir = target_dir.join("groups");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.jsonl", arm.name()));
    let lines: Vec<String> = records.iter().map(Value::to_string).collect();
    fs::write(&path, lines.join("\n") + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(tally)
}

/// An arm's Markdown rows; `model` is the engine as it loaded (the positions
/// it clamped the arm's context to, its tier and workers).
fn rows(arm: Arm, model: &str, tally: &Tally) -> String {
    let (passed, asked) = tally.passed();
    let mut text = format!(
        "\n### {} ({} positions asked)\n\n{model}\n\n| Measure | Value |\n|---|---|\n\
         | passed | {passed}/{asked} ({:.1} points) |\n",
        arm.name(),
        arm.context(),
        tally.points(),
    );
    for (cat, (p, a)) in Cat::ALL.iter().zip(tally.cats) {
        text.push_str(&format!("| {} | {p}/{a} |\n", cat.name()));
    }
    text.push_str(&format!(
        "| right app on single-app tasks | {}/{} ({:.1}%) |\n\
         | prompt positions before the request | {} |\n\
         | errors (no call) | {} |\n| context full | {} |\n| median ms a task | {} |\n",
        tally.right.0,
        tally.right.1,
        100.0 * tally.right_share(),
        tally.prompt_positions,
        tally.errors,
        tally.context_full,
        tally.median_ms(),
    ));
    text
}

/// The gate's verdict on index arm `arm` (layered or described) against
/// flat, as Markdown, and whether it passes. Both arms must answer every task
/// with a call: an error counts as a failed task, so a flat arm that errs
/// throughout (a grammar regression, a turn too long for its context) would
/// score zero and let any index score pass.
fn verdict(arm: Arm, layered: &Tally, flat: &Tally) -> (bool, String) {
    let name = arm.name();
    let trail = flat.points() - layered.points();
    let answered = |tally: &Tally| tally.errors == 0 && tally.context_full == 0;
    let (layered_answered, flat_answered) = (answered(layered), answered(flat));
    let score = trail <= MAX_TRAIL_POINTS;
    let right = layered.right_share() >= MIN_RIGHT_GROUP;
    let mark = |ok: bool| if ok { "pass" } else { "FAIL" };
    let text = format!(
        "\n### gate: {name}\n\n| Check | Value | Bar | |\n|---|---|---|---|\n\
         | flat: tasks with no call | {} ({} context full) | 0 | {} |\n\
         | {name}: tasks with no call | {} ({} context full) | 0 | {} |\n\
         | {name} against flat | {:+.1} points | >= -{MAX_TRAIL_POINTS} | {} |\n\
         | {name}: right app after the name stop | {:.1}% | >= {:.0}% | {} |\n",
        flat.errors,
        flat.context_full,
        mark(flat_answered),
        layered.errors,
        layered.context_full,
        mark(layered_answered),
        -trail,
        mark(score),
        100.0 * layered.right_share(),
        100.0 * MIN_RIGHT_GROUP,
        mark(right),
    );
    (flat_answered && layered_answered && score && right, text)
}

pub fn main(args: &[String], target_dir: &Path) -> Result<(), String> {
    let options = parse(args)?;
    let path = crate::models::stored(&options.id, options.store.as_deref())?;
    let bytes = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let groups = stubs::groups()?;
    let tasks = chosen(options.limit);
    let mut summary = format!(
        "## groups (spike S10-2)\n\n{} tasks on {} stub apps, model {}, act pack_id {:016x}, commit {}\n",
        tasks.len(),
        groups.len(),
        options.id,
        llmproto::pack::act_pack_id(),
        std::env::var("GITHUB_SHA").unwrap_or_else(|_| String::from("(local)")),
    );
    let mut tallies = Vec::new();
    for &arm in &options.arms {
        // One engine at a time: each arm has its own context.
        let loaded = actbench::load_at(bytes, &options.id, arm.context())?;
        println!("xtask: groups {}: {}", arm.name(), loaded.line);
        let tally = run_arm(&loaded, arm, &groups, &tasks, target_dir)?;
        let (passed, asked) = tally.passed();
        println!(
            "xtask: groups {}: {passed}/{asked}, right app {}/{}",
            arm.name(),
            tally.right.0,
            tally.right.1
        );
        summary.push_str(&rows(arm, &loaded.line, &tally));
        if arm == Arm::Described {
            let turn = apps::described_turn(&groups, apps::estimate);
            let plain = match turn.plain.is_empty() {
                true => String::from("none"),
                false => turn.plain.join(", "),
            };
            summary.push_str(&format!(
                "| descriptions cut to | {} words |\n\
                 | apps shown without descriptions (over the budget) | {plain} |\n",
                turn.words
            ));
        }
        tallies.push((arm, tally));
    }
    let tally_of = |arm: Arm| tallies.iter().find(|(a, _)| *a == arm).map(|(_, t)| t);
    // Each index arm against flat; the gate passes when one of them does.
    let gate = tally_of(Arm::Flat).and_then(|flat| {
        [Arm::Layered, Arm::Described]
            .into_iter()
            .filter_map(|arm| {
                let (passes, text) = verdict(arm, tally_of(arm)?, flat);
                summary.push_str(&text);
                Some(passes)
            })
            .reduce(|a, b| a || b)
    });
    write_summary(&summary, target_dir)?;
    match (options.gate, gate) {
        (true, Some(false)) => Err("the groups gate fails (see the summary)".into()),
        _ => Ok(()),
    }
}

/// The run's Markdown, to `target/groups/summary.md` and, on GitHub, the
/// job's summary.
fn write_summary(summary: &str, target_dir: &Path) -> Result<(), String> {
    let dir = target_dir.join("groups");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join("summary.md");
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
