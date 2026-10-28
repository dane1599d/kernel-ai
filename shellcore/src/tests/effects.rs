//! An app's effects (P4): a local, undoable call runs, is logged as the app's
//! and undoes; anything that would ask is refused back to the app.

use super::*;
use crate::effects::{EffectDone, MAX_EFFECTS};

const ROUND_ROBIN: &str = r#"effect 1 call({name: "set_policy", args: "{name: \"round-robin\"}"})"#;

#[test]
fn a_local_undoable_effect_runs_and_undo_puts_it_back() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let done = shell.run_effects("monitor", ROUND_ROBIN);
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].ticket, 1);
    assert!(done[0].outcome.is_ok(), "{:?}", done[0].outcome);
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    assert!(shell.eval("undo").text.starts_with("Undone: "));
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn an_effect_that_would_ask_is_refused_and_not_run() {
    let host = FakeHost::new();
    *host.procs.borrow_mut() = alloc::vec![String::from("notes")];
    let mut shell = shell(&host);
    let stop = r#"effect 4 call({name: "process_stop", args: "{name: \"notes\"}"})"#;
    let done = shell.run_effects("files", stop);
    assert!(matches!(
        &done[..],
        [EffectDone {
            ticket: 4,
            outcome: Err(_)
        }]
    ));
    assert_eq!(*host.procs.borrow(), alloc::vec![String::from("notes")]);
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|line| line.starts_with("files's effect 4 refused"))
    );
}

#[test]
fn unknown_rows_bad_arguments_and_notes_come_back_as_results() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let lines = concat!(
        "effect 1 call({name: \"format_disk\", args: \"{}\"})\n",
        "effect 2 call({name: \"set_policy\", args: \"{name: \\\"fastest\\\"}\"})\n",
        "effect 3 notify(\"saved\")\n",
        "result ok\n",
    );
    let done = shell.run_effects("settings", lines);
    assert_eq!(done.len(), 3);
    assert!(done[0].outcome.is_err());
    assert!(done[1].outcome.is_err());
    assert_eq!(done[2].outcome, Ok(String::new()));
    assert_eq!(host.policy.get(), None);
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|line| line == "settings notes: saved")
    );
}

#[test]
fn effects_past_the_limit_are_refused() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let lines: Vec<String> = (0..=MAX_EFFECTS)
        .map(|ticket| format!("effect {ticket} notify(\"n\")"))
        .collect();
    let done = shell.run_effects("files", &lines.join("\n"));
    assert_eq!(done.len(), MAX_EFFECTS + 1);
    assert!(done[..MAX_EFFECTS].iter().all(|d| d.outcome.is_ok()));
    assert!(done[MAX_EFFECTS].outcome.is_err());
}
