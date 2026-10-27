//! Arm `os`: requests for the rows that ship, answered as the llm service answers
//! `act` (`llmproto::local::act::routed` on the OS pack and grammar, one KV slot,
//! no history), then taken through the one door's name resolution
//! (`shellcore::targets::resolve`) in [`services`] and [`processes`], and checked
//! against what must run. It gates the one door's row changes, such as the stop
//! and restart rows (#99) and their name resolution (#106), at [`BAR`] of
//! [`TASKS`]. The shipping decoder is greedy, so every seed gives the same calls;
//! one seed is enough.

use super::{Loaded, Next};
use llmproto::act::{ActRequest, Call, History};
use llmproto::local::act::{self as local_act, Slots};
use serde_json::{Value, json};
use shellcore::targets::{self, Service, Unresolved, Verb};
use std::fs;
use std::path::Path;

/// What a request must come to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// This row runs (or is asked for) on this name.
    Row(&'static str, &'static str),
    /// The door answers in words (already stopped, already running).
    Say,
}

use Want::{Row, Say};

/// (request, what it must come to), in the world of [`services`] and
/// [`processes`].
pub const TASKS: &[(&str, Want)] = &[
    ("stop build-3", Row("process_stop", "build-3")),
    ("kill build-3", Row("process_stop", "build-3")),
    ("stop the process build-4", Row("process_stop", "build-4")),
    ("restart the indexer", Row("service_restart", "indexer")),
    ("stop the indexer", Row("service_stop", "indexer")),
    ("start the backup service", Row("service_start", "backup")),
    ("restart netd", Row("service_restart", "netd")),
    ("stop netd", Row("service_stop", "netd")),
    ("start netd", Say),
    ("restart the llm service", Row("service_restart", "llm")),
    // Index-only rows (ADR 0010 §6): named on an index line, so a call with
    // arguments takes the second round with the group's full lines.
    ("tune the indexer", Row("tune", "indexer")),
    (
        "roll the notes component back one version",
        Row("component_revert", "notes"),
    ),
    (
        "put back the policy the scheduler trial replaced",
        Row("tune_undo", ""),
    ),
];
/// Share of [`TASKS`] that must come right: 12 of 13.
pub const BAR: f64 = 0.9;
/// The time llm gives one call.
const TIMEOUT_MS: u64 = 60_000;

/// The supervisor's services: netd, llm and the indexer running, backup stopped.
pub fn services() -> Vec<Service> {
    [
        ("netd", 5, false),
        ("llm", 3, false),
        ("indexer", 9, false),
        ("backup", 0, true),
    ]
    .into_iter()
    .map(|(name, pid, stopped)| Service {
        name: name.into(),
        pid,
        stopped,
    })
    .collect()
}

/// The running processes: the services that run, and two builds.
pub fn processes() -> Vec<(u64, String)> {
    [
        (3, "llm"),
        (5, "netd"),
        (9, "indexer"),
        (12, "build-3"),
        (13, "build-4"),
    ]
    .into_iter()
    .map(|(pid, name)| (pid, name.into()))
    .collect()
}

/// What the door makes of `call`: a stop, start or restart row resolved by the
/// name it names, or the call as the model wrote it.
fn outcome(call: &Call) -> Result<(String, String), Unresolved> {
    let row = llmproto::pack::rows()
        .into_iter()
        .find(|row| row.name == call.name);
    // The row's subject: `name`, or the first text argument (`target`,
    // `component`).
    let name = call
        .arguments
        .get("name")
        .or_else(|| call.arguments.values().find(|v| v.is_string()))
        .and_then(Value::as_str);
    match (row.and_then(Verb::of), name) {
        (Some(verb), Some(name)) => targets::resolve(verb, name, &services(), &processes())
            .map(|found| (found.row.name.to_string(), name.to_string())),
        _ => Ok((call.name.clone(), name.unwrap_or("").to_string())),
    }
}

/// Whether `call` comes to `want`.
fn meets(call: &Call, want: Want) -> bool {
    match (outcome(call), want) {
        (Ok((row, name)), Row(want_row, want_name)) => row == want_row && name == want_name,
        (Err(Unresolved::Say(_)), Say) => true,
        (Ok((row, _)), Say) => row == "say",
        _ => false,
    }
}

/// The arm at `seed`: whether it meets [`BAR`], and its Markdown rows.
pub fn run(loaded: &Loaded, seed: u64, target_dir: &Path) -> Result<(bool, String), String> {
    let grammar = local_act::grammar().map_err(|e| e.to_string())?;
    let mut records = Vec::new();
    let mut passed = 0;
    for &(line, want) in TASKS {
        let request = ActRequest {
            line: line.into(),
            state: String::new(),
            timeout_ms: TIMEOUT_MS,
        };
        let started = std::time::Instant::now();
        let call = local_act::routed(
            &mut loaded.engine.borrow_mut(),
            &grammar,
            &request,
            &History::new(),
            Slots::ONE,
            |_| true,
            |_| Next::Continue,
        );
        let ok = call.as_ref().is_ok_and(|call| meets(call, want));
        let got = call.as_ref().map_or_else(|e| e.describe(), Call::text);
        let door = call.as_ref().ok().map(outcome);
        passed += usize::from(ok);
        println!(
            "  {} {line} -> {got}; the door: {door:?} ({} ms)",
            if ok { "ok  " } else { "FAIL" },
            started.elapsed().as_millis()
        );
        records.push(json!({
            "line": line, "got": got, "want": format!("{want:?}"),
            "door": format!("{door:?}"), "passed": ok,
        }));
    }
    let need = (TASKS.len() as f64 * BAR).ceil() as usize;
    let passes = passed >= need;
    println!(
        "xtask: actbench os seed {seed}: {} {passed}/{} requests (pass >= {need})",
        if passes { "pass" } else { "FAIL" },
        TASKS.len()
    );
    let dir = target_dir.join("actbench");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("os-s{seed}.jsonl"));
    let lines: Vec<String> = records.iter().map(Value::to_string).collect();
    fs::write(&path, lines.join("\n") + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    let mark = if passes { "yes" } else { "**no**" };
    let rows = format!(
        "\n### os seed {seed}\n\n| Bar | Value | Pass |\n|---|---|---|\n\
         | OS requests through the door (>= {need}/{}) | {passed}/{} | {mark} |\n",
        TASKS.len(),
        TASKS.len()
    );
    Ok((passes, rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(json: &str) -> Call {
        llmproto::act::parse(json).unwrap()
    }

    #[test]
    fn every_wanted_row_is_one_the_os_pack_has() {
        for (line, want) in TASKS {
            if let Row(row, _) = want {
                assert!(
                    llmproto::pack::rows().iter().any(|r| r.name == *row),
                    "{line}: {row}"
                );
            }
        }
    }

    #[test]
    fn the_door_decides_the_row_by_the_name() {
        let stop_build = call(r#"{"name":"service_stop","arguments":{"name":"build-3"}}"#);
        assert!(meets(&stop_build, Row("process_stop", "build-3")));
        let restart_backup = call(r#"{"name":"service_restart","arguments":{"name":"backup"}}"#);
        assert!(meets(&restart_backup, Row("service_start", "backup")));
        let start_netd = call(r#"{"name":"service_start","arguments":{"name":"netd"}}"#);
        assert!(meets(&start_netd, Say));
        let clarify = call(r#"{"name":"clarify","arguments":{"question":"?","options":[]}}"#);
        assert!(!meets(&clarify, Row("process_stop", "build-3")));
        let words = call(r#"{"name":"say","arguments":{"text":"It already runs."}}"#);
        assert!(meets(&words, Say));
        let tune = call(r#"{"name":"tune","arguments":{"target":"indexer","intent":null}}"#);
        assert!(meets(&tune, Row("tune", "indexer")));
        let revert =
            call(r#"{"name":"component_revert","arguments":{"component":"notes","back":1}}"#);
        assert!(meets(&revert, Row("component_revert", "notes")));
        assert!(meets(
            &call(r#"{"name":"tune_undo","arguments":{}}"#),
            Row("tune_undo", "")
        ));
    }
}
