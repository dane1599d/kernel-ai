//! Tiers, undo, show and bare answers: what the user sees when a script runs at
//! once, asks, or is refused.

use super::scripts::{checks, ran, with_answers};
use super::*;
use crate::script::HELPERS;
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
fn every_function_a_script_can_call_has_a_tier_and_no_row_names_a_missing_one() {
    let callable = || API.iter().map(|f| f.name).chain(HELPERS.iter().copied());
    for function in callable() {
        assert!(
            TIERS.iter().any(|(name, _)| *name == function),
            "{function} has no tier"
        );
    }
    for (name, _) in TIERS {
        assert!(callable().any(|f| f == *name), "{name} cannot be called");
    }
    // The helpers touch nothing, so they are run-tier.
    for helper in HELPERS {
        assert_eq!(tiers::of(TIERS, helper), Tier::Run, "{helper}");
    }
    assert_eq!(tiers::of(TIERS, "kill"), Tier::Ask);
    let checked = checks("print(cpus());\nset_policy(\"rr\");").unwrap();
    assert_eq!(
        shell(&FakeHost::new()).tier_of("use round robin", &checked),
        (Tier::Run, vec!["cpus", "set_policy"])
    );
    assert_eq!(
        asking(vec![]).1.tier_of("use round robin", &checked),
        (Tier::Ask, vec!["set_policy"])
    );
}

#[test]
fn a_change_runs_only_when_the_request_names_it_and_its_value() {
    let tier = |request: &str, script: &str| {
        let checked = checks(script).unwrap();
        shell(&FakeHost::new()).tier_of(request, &checked).0
    };
    for (request, script) in [
        ("use Round Robin", SWITCH),
        ("go round-robin", "set_policy(\"rr\");"),
        ("back to interactive", "set_policy(\"interactive\");"),
    ] {
        assert_eq!(tier(request, script), Tier::Run, "{request}");
    }
    // The cue without the value, or a negated cue, is not asking for it.
    for request in ["show the Policy", "don't switch to round robin"] {
        assert_eq!(tier(request, SWITCH), Tier::Ask, "{request}");
    }
    // No line makes a never-tier function run.
    let never = Shell::with_tiers(FakeHost::new(), DEFAULT_CONFIG, policy_at(Tier::Never));
    let checked = checks(SWITCH).unwrap();
    assert_eq!(
        never.tier_of("set the policy to round robin", &checked).0,
        Tier::Never
    );
}

#[test]
fn a_change_the_model_added_to_a_read_asks_and_says_why() {
    let script = "print(cpus());\nset_policy(\"rr\");";
    let (host, mut shell) = with_answers(vec![answer(script, "Counts CPUs.")]);
    let reply = shell.eval("how many cpus are there");
    assert_eq!(reply.confirm, Some(1));
    assert_eq!(
        reply.text,
        "script 1: Counts CPUs.\nIt changes set_policy, which the request did not ask for. Go ahead? y/n, or show"
    );
    assert_eq!(host.policy.get(), None);
    assert_eq!(shell.eval("n"), ok("script 1 not run"));
    assert_eq!(host.policy.get(), None);
    assert!(
        shell
            .eval("show")
            .text
            .ends_with("\nasked first: the request did not ask for set_policy")
    );
}

/// A call to each run-tier function that changes anything, and a request that
/// names it.
const CALLS: &[(&str, &str, &str)] = &[(
    "set_policy",
    "set_policy(\"rr\");",
    "change the policy to round robin",
)];

#[test]
fn every_run_tier_change_records_how_to_undo_it() {
    let changes = API.iter().filter(|f| f.effect() == Effect::Change);
    for function in changes.filter(|f| tiers::of(TIERS, f.name) == Tier::Run) {
        let (_, call, request) = CALLS
            .iter()
            .find(|(name, ..)| *name == function.name)
            .unwrap_or_else(|| panic!("add a call to {} to CALLS", function.name));
        let (_host, mut shell) = with_answers(vec![answer(call, "Changes it.")]);
        let reply = shell.eval(&format!("? {request}"));
        assert!(reply.text.ends_with(". undo"), "{reply:?}");
        assert!(shell.eval("undo").ok, "{}", function.name);
    }
}

#[test]
fn undo_puts_the_last_change_back_and_undo_again_redoes() {
    let (host, mut shell) = with_answers(vec![answer(SWITCH, "Switches to round-robin.")]);
    assert_eq!(
        ran(&shell.eval("use round robin")),
        "Done: policy round-robin (was interactive). undo"
    );
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    assert_eq!(shell.eval("undo"), ok("Undone: policy back to interactive"));
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
    // An undo is an entry too, so undo again redoes.
    assert_eq!(shell.eval("undo"), ok("Undone: policy back to round-robin"));
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|line| line == "undo #2: undid #1")
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
    // Tried once more after 100 ms, then left for another `undo`.
    assert_eq!(*host.slept.borrow(), [100]);
    host.policy_busy.set(false);
    assert!(shell.eval("undo").ok);
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn an_undo_refused_as_too_soon_is_tried_once_more() {
    let (host, mut shell) = with_answers(vec![answer(SWITCH, "Switches.")]);
    shell.eval("use round robin");
    host.busy_once.set(true);
    assert_eq!(shell.eval("undo"), ok("Undone: policy back to interactive"));
    assert_eq!(*host.slept.borrow(), [100]);
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn undo_asks_before_overwriting_a_later_change() {
    let answers = vec![answer(SWITCH, "Switches."), answer(SWITCH, "Switches.")];
    let (host, mut shell) = with_answers(answers);
    shell.eval("use round robin");
    // The user switches back by hand; undo would now overwrite that.
    host.policy.set(Some(policy::INTERACTIVE));
    let asked = "The policy changed since (now interactive); put interactive back anyway? y/n";
    assert_eq!(shell.eval("undo"), ok(asked));
    let switches = host.switches.get();
    assert_eq!(shell.eval("n"), ok("Not undone; `undo` asks again"));
    assert_eq!(host.switches.get(), switches);
    assert_eq!(shell.eval("y"), Reply::err("nothing to answer"));
    // Another line leaves the undo for later too.
    assert_eq!(shell.eval("undo"), ok(asked));
    assert_eq!(shell.eval("1 + 1"), ok("2"));
    assert_eq!(shell.eval("undo"), ok(asked));
    assert_eq!(shell.eval("Yes"), ok("Undone: policy back to interactive"));
    assert_eq!(host.switches.get(), switches + 1);
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|l| l == "undo #1 asks: changed since (now interactive)")
    );
    // Unmoved, it does not ask: the redo puts round-robin back at once.
    assert_eq!(shell.eval("undo"), ok("Undone: policy back to round-robin"));
}

#[test]
fn a_resource_changed_again_keeps_its_first_value() {
    // The snapshot is taken at the first change, so undo goes straight back to it.
    let script = "set_policy(\"rr\");\nset_policy(\"interactive\");\nset_policy(\"rr\");";
    let (host, mut shell) = with_answers(vec![answer(script, "Flips.")]);
    let reply = shell.eval("go round robin, then interactive, then round robin");
    assert_eq!(
        reply,
        ok("Done: policy round-robin (was interactive). undo")
    );
    assert_eq!(shell.eval("undo"), ok("Undone: policy back to interactive"));
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn a_run_that_fails_after_a_change_is_rolled_back() {
    // The trial's stand-in has two CPUs; the fake host has four.
    let script = "set_policy(\"rr\");\nprint(10 / (cpus() - 4));";
    let (host, mut shell) = with_answers(vec![answer(script, "Switches and divides.")]);
    let reply = shell.eval("switch the policy to round robin then divide please");
    assert!(!reply.ok);
    assert_eq!(
        reply.text,
        "Nothing changed: Division by zero: 10 / 0 (line 2, position 10)"
    );
    // The switch was put back at once, so there is nothing left to undo.
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
    assert_eq!(shell.eval("undo"), Reply::err("nothing to undo"));
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
        ok("Done: policy round-robin (was interactive). undo")
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

/// #15's table (`shellcore/src/tiers.rs` on main at 57d7120), now generated from
/// the action registry with the same tiers.
#[test]
fn the_generated_tier_table_is_the_seamless_actions_one() {
    let before = [
        ("cpus", Tier::Run),
        ("memory", Tier::Run),
        ("uptime_ms", Tier::Run),
        ("processes", Tier::Run),
        ("threads", Tier::Run),
        ("get_policy", Tier::Run),
        ("set_policy", Tier::Run),
        ("fixed", Tier::Run),
        ("floor", Tier::Run),
    ];
    assert_eq!(TIERS, before);
    assert_eq!(HELPERS, ["fixed", "floor"]);
}

#[test]
fn every_script_api_row_has_a_binding_and_help_lists_it() {
    let host = FakeHost::new();
    let shared: Rc<dyn Host> = host.clone();
    assert!(crate::script::unbound(&shared).is_empty());
    let mut shell = shell(&host);
    let help = shell.eval("help").text;
    for row in API {
        let signature = alloc::format!("{}: {}", row.signature(), row.doc);
        assert!(help.contains(&signature), "{signature}");
    }
}
