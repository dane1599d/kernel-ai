//! Arm `shipped`: the 190 tasks ([`tasks::TASKS`]) answered as the llm service
//! answers `act`, on the pack that ships (`llmproto::pack`, the OS's rows and
//! grammar, `llmproto::local::act::routed`), beside the gated `act` arm, which
//! answers them on the 40-row mock pack ([`super::render`]). Report only: it
//! gates nothing, so the founder can see the gap between the mock and the
//! product before deciding whether the gate should move here.
//!
//! A task whose expected row the OS does not have (such as `component_apply`)
//! is skipped and counted. A `script` call is scored on its routing: llm writes
//! the code next with the script prompt, which the fluency gate measures. An
//! ambiguous task passes on `clarify` or a row the OS asks before running, as
//! the gated arm's does. History turns go in as llm's conversation memory.

use super::tasks::{self, Cat, Expect, Task};
use super::{Loaded, Next};
use llmproto::act::{ActRequest, Call, History, Turn};
use llmproto::local::act::{self as local_act, Slots};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// The time llm gives one call.
const TIMEOUT_MS: u64 = 60_000;
/// History's token budget, as llm's `HISTORY_BUDGET`.
const HISTORY_BUDGET: usize = 512;

/// Whether the OS pack has a row of this name.
fn os_row(name: &str) -> bool {
    matches!(name, "say" | "clarify" | "script" | "undo")
        || llmproto::pack::rows().iter().any(|row| row.name == name)
}

/// The rows a task's expectations name, each of which the OS must have.
fn named(expect: &Expect) -> Vec<String> {
    match expect {
        Expect::Call(text) => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|v| v["name"].as_str().map(String::from))
            .into_iter()
            .collect(),
        Expect::Name(name) | Expect::Uses(name) => vec![name.to_string()],
        Expect::Script(names) => names.iter().map(|n| n.to_string()).collect(),
        Expect::Say | Expect::Clarify => Vec::new(),
    }
}

/// Whether the OS can answer `task` at all: every row some expectation names.
fn answerable(task: &Task) -> bool {
    task.expect
        .iter()
        .any(|e| named(e).iter().all(|name| os_row(name)))
}

/// Whether `call` meets `expect`, a script scored on its routing.
fn matches(expect: &Expect, call: &Value) -> bool {
    let name = call["name"].as_str().unwrap_or("");
    match expect {
        Expect::Call(text) => serde_json::from_str::<Value>(text).is_ok_and(|want| *call == want),
        Expect::Name(want) => name == *want,
        Expect::Uses(want) => name == *want || name == "script",
        Expect::Script(_) => name == "script",
        Expect::Say => name == "say",
        Expect::Clarify => name == "clarify",
    }
}

/// Whether the OS asks before running `name`.
fn asks(name: &str) -> bool {
    llmproto::pack::rows()
        .iter()
        .find(|row| row.name == name)
        .is_some_and(|row| agentabi::derive(row) == agentabi::Tier::Ask)
}

fn passed(task: &Task, call: &Value) -> bool {
    let name = call["name"].as_str().unwrap_or("");
    match task.cat {
        Cat::Ambiguous => name == "clarify" || asks(name),
        _ => task.expect.iter().any(|e| matches(e, call)),
    }
}

fn history(task: &Task) -> History {
    let mut history = History::new();
    for turn in task.history {
        let turn = Turn {
            line: turn.line.into(),
            call: turn.call.into(),
            result: turn.result.into(),
        };
        let cost = (turn.line.len() + turn.call.len() + turn.result.len()) / 3 + 8;
        history.push(turn, cost, HISTORY_BUDGET);
    }
    history
}

/// The arm at `seed`, reported: always "passes", and its Markdown rows.
pub fn run(loaded: &Loaded, seed: u64, target_dir: &Path) -> Result<(bool, String), String> {
    let grammar = super::apps::grammar()?;
    let mut records = Vec::new();
    // (passed, answered, skipped) for each category.
    let mut tally = [(0usize, 0usize, 0usize); Cat::ALL.len()];
    for task in tasks::TASKS {
        let cat = Cat::ALL.iter().position(|c| *c == task.cat).unwrap_or(0);
        if !answerable(task) {
            tally[cat].2 += 1;
            records.push(json!({"cat": task.cat.name(), "line": task.line, "skipped": true}));
            continue;
        }
        let request = ActRequest {
            line: task.line.into(),
            state: task.state.into(),
            timeout_ms: TIMEOUT_MS,
            read: None,
            apps: String::new(),
        };
        let call = local_act::routed(
            &mut loaded.engine.borrow_mut(),
            &grammar,
            &request,
            &history(task),
            Slots::ONE,
            |_| true,
            |_| Next::Continue,
        );
        let value = call
            .as_ref()
            .ok()
            .and_then(|call| serde_json::from_str::<Value>(&call.text()).ok());
        let ok = value.as_ref().is_some_and(|v| passed(task, v));
        let got = call.as_ref().map_or_else(|e| e.describe(), Call::text);
        tally[cat].0 += usize::from(ok);
        tally[cat].1 += 1;
        if !ok {
            println!("  FAIL {:<10} {} -> {got}", task.cat.name(), task.line);
        }
        records.push(json!({"cat": task.cat.name(), "line": task.line, "got": got, "passed": ok}));
    }
    let mut rows = format!(
        "\n### shipped seed {seed} (report only: the shipped pack)\n\n\
         | Category | Passed | Skipped (row not in the OS) |\n|---|---|---|\n"
    );
    for (cat, (passed, answered, skipped)) in Cat::ALL.iter().zip(tally) {
        println!(
            "  shipped {:<10} {passed}/{answered}, {skipped} skipped",
            cat.name()
        );
        rows.push_str(&format!(
            "| {} | {passed}/{answered} | {skipped} |\n",
            cat.name()
        ));
    }
    let (passed, answered, skipped) = tally
        .iter()
        .fold((0, 0, 0), |(p, a, s), t| (p + t.0, a + t.1, s + t.2));
    println!(
        "xtask: actbench shipped seed {seed}: {passed}/{answered} on the shipped pack, \
         {skipped} skipped (report only)"
    );
    let dir = target_dir.join("actbench");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("shipped-s{seed}.jsonl"));
    let lines: Vec<String> = records.iter().map(Value::to_string).collect();
    fs::write(&path, lines.join("\n") + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((true, rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_for_a_row_the_os_lacks_is_skipped() {
        let apply = tasks::TASKS
            .iter()
            .find(|t| t.line == "apply sched")
            .unwrap();
        assert!(!answerable(apply));
        let start = tasks::TASKS.iter().find(|t| t.line == "start llm").unwrap();
        assert!(answerable(start));
        let skipped = tasks::TASKS.iter().filter(|t| !answerable(t)).count();
        assert!(skipped < tasks::TASKS.len() / 2, "{skipped} skipped");
    }

    #[test]
    fn scripts_are_scored_on_their_routing_and_ambiguity_on_a_question_or_an_ask() {
        let script = json!({"name": "script", "arguments": {}});
        assert!(matches(&Expect::Script(&["cpus"]), &script));
        assert!(matches(&Expect::Uses("cpus"), &script));
        let ambiguous = tasks::TASKS
            .iter()
            .find(|t| t.cat == Cat::Ambiguous)
            .unwrap();
        assert!(passed(
            ambiguous,
            &json!({"name": "clarify", "arguments": {}})
        ));
        assert!(passed(
            ambiguous,
            &json!({"name": "service_restart", "arguments": {"name": "netd"}})
        ));
        assert!(!passed(
            ambiguous,
            &json!({"name": "cpus", "arguments": {}})
        ));
    }
}
