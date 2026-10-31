//! Arm `os`: requests for the rows that ship, answered as the llm service answers
//! `act` (`llmproto::local::act::routed` on the OS pack and grammar, one KV slot,
//! no history), then taken through the one door's name resolution
//! (`shellcore::targets::resolve`) in [`services`] and [`processes`], and checked
//! against what must run. It gates the one door's row changes, such as the stop
//! and restart rows (#99) and their name resolution (#106), and which groups
//! the pack shows by name only (ADR 0010 §6), at [`BAR`] of [`TASKS`]. Each
//! request also reports its decode rounds (two for an index-only row with
//! arguments) and the tokens it computed. The shipping decoder is greedy, so
//! every seed gives the same calls; one seed is enough.

use super::{Loaded, Next};
use infer::chat::Step;
use llmproto::act::{ActRequest, Call, History};
use llmproto::local::act::{self as local_act, Slots};
use serde_json::{Value, json};
use shellcore::targets::{self, Service, Unresolved, Verb};
use std::cell::Cell;
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
    ("tune the indexer", Row("tune", "indexer")),
    (
        "put back the policy the scheduler trial replaced",
        Row("tune_undo", ""),
    ),
    // Packages and files (P2), the groups most likely to be shown by name only.
    ("install htop", Row("pkg_install", "htop")),
    ("remove the notes package", Row("pkg_remove", "notes")),
    (
        "search the package repository for htop",
        Row("pkg_search", "htop"),
    ),
    ("show me what is in notes.txt", Row("fs_read", "notes.txt")),
    ("rename todo.txt to done.txt", Row("fs_move", "todo.txt")),
    ("move old.txt to the trash", Row("fs_trash", "old.txt")),
    // Index-only (ADR 0010 §6): named on an index line, so a call with
    // arguments takes the second round with the group's full lines.
    (
        "roll the notes component back one version",
        Row("component_revert", "notes"),
    ),
];
/// Share of [`TASKS`] that must come right: 18 of 19.
pub const BAR: f64 = 0.9;

/// Arm `apps` (P4 task 15): requests for the shipped apps' rows, which the
/// apps turn names and the second round writes (`actbench::apps`). Reported
/// at [`BAR`], not gated yet; `notes` is the one app whose rows load today.
pub const APP_TASKS: &[(&str, Want)] = &[
    ("add a note saying buy milk", Row("notes_add", "buy milk")),
    (
        "make a note: call the dentist",
        Row("notes_add", "call the dentist"),
    ),
    ("how many notes do I have?", Row("notes_count", "")),
    ("find my notes that mention milk", Row("notes_find", "milk")),
    ("do I have any notes about eggs?", Row("notes_find", "eggs")),
    ("which version of notes is running?", Row("notes_about", "")),
    // The OS's rows still answer their own requests with the apps shown.
    ("show me what is in notes.txt", Row("fs_read", "notes.txt")),
    ("remove the notes package", Row("pkg_remove", "notes")),
];
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

/// The arguments that name a row's subject, in the order they are looked for.
const SUBJECTS: [&str; 6] = ["name", "target", "component", "path", "from", "text"];

/// What the door makes of `call`: a stop, start or restart row resolved by the
/// name it names, or the call as the model wrote it.
fn outcome(call: &Call) -> Result<(String, String), Unresolved> {
    let row = llmproto::pack::rows()
        .into_iter()
        .find(|row| row.name == call.name);
    // The row's subject: `name`, `target`, `component`, `path`, `from` or
    // `text`, else the first text argument (`tune`'s `intent` sorts before its
    // `target`, run 40536225586). A path is named from the home folder, and
    // `./notes.txt` is `notes.txt` (run 40563541080).
    let name = SUBJECTS
        .iter()
        .find_map(|key| call.arguments.get(*key).filter(|v| v.is_string()))
        .or_else(|| call.arguments.values().find(|v| v.is_string()))
        .and_then(Value::as_str)
        .map(|name| {
            name.trim_start_matches("/home/user/")
                .trim_start_matches("./")
        });
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
    run_tasks(loaded, seed, target_dir, "os", TASKS)
}

/// Arm `apps` at `seed`, reported: whether it met [`BAR`], and its rows.
pub fn run_apps(loaded: &Loaded, seed: u64, target_dir: &Path) -> Result<(bool, String), String> {
    run_tasks(loaded, seed, target_dir, "apps", APP_TASKS)
}

fn run_tasks(
    loaded: &Loaded,
    seed: u64,
    target_dir: &Path,
    arm: &str,
    tasks: &[(&str, Want)],
) -> Result<(bool, String), String> {
    let grammar = super::apps::grammar()?;
    let mut records = Vec::new();
    let mut passed = 0;
    let (mut all_rounds, mut all_tokens) = (0, 0);
    for &(line, want) in tasks {
        let request = ActRequest {
            line: line.into(),
            state: String::new(),
            timeout_ms: TIMEOUT_MS,
            read: None,
            apps: String::new(),
        };
        let started = std::time::Instant::now();
        // Rounds are the decodes (each starts with its cache count); tokens are
        // those the model computed, prompt and answer.
        let (rounds, tokens) = (Cell::new(0), Cell::new(0));
        let call = local_act::routed(
            &mut loaded.engine.borrow_mut(),
            &grammar,
            &request,
            &History::new(),
            Slots::ONE,
            |_| true,
            |step| {
                match step {
                    Step::Cached(_) => rounds.set(rounds.get() + 1),
                    Step::Prompt | Step::Token(_) => tokens.set(tokens.get() + 1),
                }
                Next::Continue
            },
        );
        let (rounds, tokens) = (rounds.get(), tokens.get());
        all_rounds += rounds;
        all_tokens += tokens;
        let ok = call.as_ref().is_ok_and(|call| meets(call, want));
        let got = call.as_ref().map_or_else(|e| e.describe(), Call::text);
        let door = call.as_ref().ok().map(outcome);
        passed += usize::from(ok);
        println!(
            "  {} {line} -> {got}; the door: {door:?} ({} ms, {rounds} rounds, {tokens} tokens)",
            if ok { "ok  " } else { "FAIL" },
            started.elapsed().as_millis()
        );
        records.push(json!({
            "line": line, "got": got, "want": format!("{want:?}"),
            "door": format!("{door:?}"), "passed": ok,
            "rounds": rounds, "tokens": tokens,
        }));
    }
    let need = (tasks.len() as f64 * BAR).ceil() as usize;
    let passes = passed >= need;
    let per = |total: usize| total as f64 / tasks.len() as f64;
    println!(
        "xtask: actbench {arm} seed {seed}: {} {passed}/{} requests (pass >= {need}); \
         {:.2} rounds and {:.0} tokens a request",
        if passes { "pass" } else { "FAIL" },
        tasks.len(),
        per(all_rounds),
        per(all_tokens)
    );
    let dir = target_dir.join("actbench");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{arm}-s{seed}.jsonl"));
    let lines: Vec<String> = records.iter().map(Value::to_string).collect();
    fs::write(&path, lines.join("\n") + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    let mark = if passes { "yes" } else { "**no**" };
    let rows = format!(
        "\n### {arm} seed {seed}\n\n| Bar | Value | Pass |\n|---|---|---|\n\
         | Requests through the door (>= {need}/{}) | {passed}/{} | {mark} |\n\
         | Decode rounds a request | {:.2} | |\n\
         | Tokens computed a request | {:.0} | |\n",
        tasks.len(),
        tasks.len(),
        per(all_rounds),
        per(all_tokens)
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
    fn every_wanted_app_row_is_a_shipped_apps_or_the_os_packs() {
        let groups = super::super::apps::shipped().unwrap();
        for (line, want) in APP_TASKS {
            if let Row(row, _) = want {
                let shipped = groups.iter().any(|g| g.rows.iter().any(|r| r.name == *row));
                let os = llmproto::pack::rows().iter().any(|r| r.name == *row);
                assert!(shipped || os, "{line}: {row}");
            }
        }
        let add = call(r#"{"name":"notes_add","arguments":{"text":"buy milk"}}"#);
        assert!(meets(&add, Row("notes_add", "buy milk")));
        let find = call(r#"{"name":"notes_find","arguments":{"word":"milk"}}"#);
        assert!(meets(&find, Row("notes_find", "milk")));
        let count = call(r#"{"name":"notes_count","arguments":{}}"#);
        assert!(meets(&count, Row("notes_count", "")));
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
        let tune = call(r#"{"name":"tune","arguments":{"intent":"faster","target":"indexer"}}"#);
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
