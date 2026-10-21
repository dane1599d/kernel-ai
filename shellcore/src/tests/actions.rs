//! Tiers, undo, show and bare answers: what the user sees when a script runs at
//! once, asks, or is refused.

use super::scripts::{ran, with_answers};
use super::*;
use crate::tiers::{self, TIERS, Tier};
use alloc::vec;
use llmproto::script::{API, Effect};

fn answer(script: &str, summary: &str) -> Result<Answer, AnswerError> {
    Ok(Answer {
        summary: String::from(summary),
        script: String::from(script),
    })
}

/// The shipped table with `set_policy` moved to `tier`, so the ask and never
/// paths can be tested with a real function.
fn policy_at(tier: Tier) -> &'static [(&'static str, Tier)] {
    let table: Vec<(&'static str, Tier)> = TIERS
        .iter()
        .map(|&(name, t)| (name, if name == "set_policy" { tier } else { t }))
        .collect();
    Vec::leak(table)
}

fn asking(answers: Vec<Result<Answer, AnswerError>>) -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    host.answers.borrow_mut().extend(answers);
    let shell = Shell::with_tiers(host.clone(), DEFAULT_CONFIG, policy_at(Tier::Ask));
    (host, shell)
}

const SWITCH: &str = "set_policy(\"round-robin\");";

#[test]
fn every_api_function_has_a_tier_and_no_row_names_a_missing_one() {
    for function in API {
        assert!(
            TIERS.iter().any(|(name, _)| *name == function.name),
            "{} has no tier",
            function.name
        );
    }
    for (name, _) in TIERS {
        assert!(
            API.iter().any(|f| f.name == *name),
            "{name} is not in the API"
        );
    }
    assert_eq!(tiers::of(TIERS, "kill"), Tier::Ask);
    assert_eq!(
        tiers::of_script(TIERS, &["cpus", "set_policy"]),
        (Tier::Run, vec!["cpus", "set_policy"])
    );
    assert_eq!(
        tiers::of_script(policy_at(Tier::Ask), &["cpus", "set_policy"]),
        (Tier::Ask, vec!["set_policy"])
    );
}

/// A call to each run-tier function that changes anything.
const CALLS: &[(&str, &str)] = &[("set_policy", "set_policy(\"rr\");")];

#[test]
fn every_run_tier_change_records_how_to_undo_it() {
    let changes = API.iter().filter(|f| f.effect == Effect::Change);
    for function in changes.filter(|f| tiers::of(TIERS, f.name) == Tier::Run) {
        let (_, call) = CALLS
            .iter()
            .find(|(name, _)| *name == function.name)
            .unwrap_or_else(|| panic!("add a call to {} to CALLS", function.name));
        let (_host, mut shell) = with_answers(vec![answer(call, "Changes it.")]);
        let reply = shell.eval("? change it");
        assert!(reply.text.ends_with("(undo: `undo`)"), "{reply:?}");
        assert!(shell.eval("undo").ok, "{}", function.name);
    }
}

#[test]
fn undo_puts_the_last_change_back_once() {
    let (host, mut shell) = with_answers(vec![answer(SWITCH, "Switches to round-robin.")]);
    assert_eq!(
        ran(&shell.eval("use round robin")),
        "Done: policy round-robin (was interactive) (undo: `undo`)"
    );
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    assert_eq!(shell.eval("undo"), ok("Undone: policy back to interactive"));
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
    assert_eq!(shell.eval("undo"), Reply::err("nothing to undo"));
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|line| line == "script 1 undone")
    );
}

#[test]
fn an_undo_the_kernel_refuses_stays_for_another_try() {
    let (host, mut shell) = with_answers(vec![answer(SWITCH, "Switches.")]);
    shell.eval("use round robin");
    host.policy_busy.set(true);
    assert_eq!(
        shell.eval("undo"),
        Reply::err("cannot switch the policy back yet (Busy); try undo again")
    );
    host.policy_busy.set(false);
    assert!(shell.eval("undo").ok);
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn two_switches_undo_newest_first_to_the_first_policy() {
    let script = "set_policy(\"rr\");\nset_policy(\"interactive\");\nset_policy(\"rr\");";
    let (host, mut shell) = with_answers(vec![answer(script, "Flips.")]);
    let reply = shell.eval("flip the policy about");
    assert!(
        reply
            .text
            .starts_with("Done: policy round-robin (was interactive); ")
    );
    assert_eq!(
        shell.eval("undo").text,
        "Undone: policy back to interactive; policy back to round-robin; policy back to interactive"
    );
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn a_run_that_fails_after_a_change_says_so_and_can_be_undone() {
    // The trial's stand-in has two CPUs; the fake host has four.
    let script = "set_policy(\"rr\");\nprint(10 / (cpus() - 4));";
    let (host, mut shell) = with_answers(vec![answer(script, "Switches and divides.")]);
    let reply = shell.eval("switch then divide please");
    assert!(!reply.ok);
    assert!(
        reply.text.ends_with(
            "\nChanged before the error: policy round-robin (was interactive) (undo: `undo`)"
        ),
        "{reply:?}"
    );
    assert!(shell.eval("undo").ok);
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn a_run_with_no_output_and_no_change_says_done() {
    let script = "if get_policy() != \"interactive\" {\n    set_policy(\"interactive\");\n}";
    let (_host, mut shell) = with_answers(vec![answer(script, "Uses interactive if needed.")]);
    assert_eq!(
        ran(&shell.eval("use interactive scheduling")),
        "Done: Uses interactive if needed."
    );
    assert_eq!(shell.eval("undo"), Reply::err("nothing to undo"));
}

#[test]
fn show_and_why_print_the_last_script_and_why_it_ran() {
    let (_host, mut shell) = with_answers(vec![answer(SWITCH, "Switches.")]);
    assert_eq!(shell.eval("show"), Reply::err("no script yet"));
    shell.eval("use round robin");
    let shown = "script 1: CHANGES set_policy\nmodel says: Switches.\n 1 | set_policy(\"round-robin\");\nran at once: its changes can be undone";
    assert_eq!(shell.eval("show"), ok(shown));
    assert_eq!(shell.eval("why"), ok(shown));
    shell.eval("let show = 5");
    assert_eq!(shell.eval("show"), ok("5"));
}

#[test]
fn an_ask_tier_script_asks_one_short_question_and_runs_on_y() {
    let (host, mut shell) = asking(vec![answer(SWITCH, "Switches to round-robin.")]);
    let reply = shell.eval("switch to round robin");
    assert_eq!(reply.confirm, Some(1));
    assert_eq!(
        reply.text,
        "script 1: Switches to round-robin.\nIt changes set_policy (no undo). Go ahead? y/n, or show"
    );
    assert_eq!(
        reply.to_wire().split_once(' ').map(|(kind, _)| kind),
        Some("confirm")
    );
    assert_eq!(host.policy.get(), None);
    // The undo offered is whatever the run recorded, whatever the tier.
    assert_eq!(
        shell.eval("Y"),
        ok("Done: policy round-robin (was interactive) (undo: `undo`)")
    );
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    let audit = host.audit.borrow();
    assert!(
        audit
            .iter()
            .any(|l| l == "script 1 tier ask (set_policy); asking")
    );
    assert!(audit.iter().any(|l| l == "script 1 confirmed; running it"));
    assert!(
        audit
            .iter()
            .any(|l| { l == "script 1 set_policy (ask): policy round-robin (was interactive)" })
    );
}

#[test]
fn n_or_anything_but_yes_declines_and_changes_nothing() {
    let (host, mut shell) = asking(vec![
        answer(SWITCH, "Switches."),
        answer(SWITCH, "Switches."),
    ]);
    shell.eval("? switch to round robin");
    assert_eq!(shell.eval("n"), ok("script 1 not run"));
    assert_eq!(host.policy.get(), None);
    shell.eval("? switch to round robin");
    assert_eq!(shell.eval("answer 2 "), ok("script 2 not run"));
    assert_eq!(host.policy.get(), None);
    assert!(host.audit.borrow().iter().any(|l| l == "script 1 declined"));
    assert_eq!(shell.eval("yes"), Reply::err("nothing to answer"));
}

#[test]
fn show_at_the_question_keeps_it_waiting() {
    let (host, mut shell) = asking(vec![answer(SWITCH, "Switches.")]);
    shell.eval("? switch to round robin");
    let shown = shell.eval("show");
    assert_eq!(shown.confirm, Some(1));
    assert_eq!(
        shown.text,
        "script 1: CHANGES set_policy\nmodel says: Switches.\n 1 | set_policy(\"round-robin\");\nscript 1: Switches.\nIt changes set_policy (no undo). Go ahead? y/n, or show"
    );
    assert_eq!(shell.eval("answer 1 why").confirm, Some(1));
    assert!(
        shell
            .eval("answer 1 yes")
            .text
            .starts_with("Done: policy round-robin")
    );
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
}

#[test]
fn a_stale_or_wrong_id_or_another_line_or_time_runs_nothing() {
    let (host, mut shell) = asking(vec![
        answer(SWITCH, "Switches."),
        answer(SWITCH, "Switches."),
        answer(SWITCH, "Switches."),
    ]);
    shell.eval("? switch please");
    assert_eq!(shell.eval("answer 7 yes"), Reply::err("nothing to confirm"));
    assert_eq!(shell.eval("1 + 1"), ok("2"));
    assert_eq!(shell.eval("answer 1 yes"), Reply::err("nothing to confirm"));
    assert_eq!(shell.eval("y"), Reply::err("nothing to answer"));
    shell.eval("? switch again please");
    host.clock.set(host.clock.get() + 200_000_000_000);
    assert_eq!(shell.eval("y"), Reply::err("the script expired; ask again"));
    assert_eq!(host.policy.get(), None);
    // A variable named y is still a variable.
    shell.eval("? switch once more please");
    shell.eval("let y = 3");
    assert_eq!(shell.eval("y"), ok("3"));
    assert_eq!(host.policy.get(), None);
}

#[test]
fn a_never_tier_function_is_refused_and_sent_back_once() {
    let host = FakeHost::new();
    host.answers.borrow_mut().extend([
        answer(SWITCH, "Switches."),
        answer("print(get_policy());", "Prints the policy."),
    ]);
    let mut shell = Shell::with_tiers(host.clone(), DEFAULT_CONFIG, policy_at(Tier::Never));
    assert_eq!(ran(&shell.eval("? switch to round robin")), "interactive\n");
    assert_eq!(host.policy.get(), None);
    let (_, why) = host.script_requests.borrow()[1].repair.clone().unwrap();
    assert_eq!(why, "set_policy may not be used in scripts here");
    host.answers
        .borrow_mut()
        .extend([answer(SWITCH, "Switches."), answer(SWITCH, "Switches.")]);
    let reply = shell.eval("? switch to round robin");
    assert_eq!(
        reply,
        Reply::err("gave up after one repair: set_policy may not be used in scripts here")
    );
    assert_eq!(host.policy.get(), None);
}
