//! The LLM script path: routing, the strict checks, repair, confirmation and runs.

use super::*;
use crate::script::{check, names_in};
use alloc::vec;

fn answer(script: &str, summary: &str) -> Result<Answer, AnswerError> {
    Ok(Answer {
        summary: String::from(summary),
        script: String::from(script),
    })
}

fn with_answers(answers: Vec<Result<Answer, AnswerError>>) -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    host.answers.borrow_mut().extend(answers);
    let shell = shell(&host);
    (host, shell)
}

#[test]
fn a_script_is_shown_in_full_and_runs_only_after_yes() {
    let (host, mut shell) = with_answers(vec![answer(
        "print(\"CPUs: \" + cpus());",
        "Prints the CPUs.",
    )]);
    let reply = shell.eval("? how many cpus are there");
    assert_eq!(reply.confirm, Some(1));
    assert_eq!(
        reply.text,
        "script 1: reads cpus\nmodel says: Prints the CPUs.\n 1 | print(\"CPUs: \" + cpus());\nrun it? yes/no"
    );
    assert_eq!(
        reply.to_wire().split_once(' ').map(|(kind, _)| kind),
        Some("confirm")
    );
    assert_eq!(
        host.script_requests.borrow()[0].request,
        "how many cpus are there"
    );
    assert_eq!(shell.eval("answer 1 yes"), ok("CPUs: 4\n"));
    assert_eq!(shell.eval("answer 1 yes"), Reply::err("nothing to confirm"));
    assert!(
        shell
            .eval("log")
            .text
            .contains("#1 how many cpus are there: ran")
    );
    assert!(shell.eval("log 1").text.contains("print(\"CPUs: \""));
}

#[test]
fn anything_but_yes_declines_and_changes_nothing() {
    let (host, mut shell) = with_answers(vec![
        answer("set_policy(\"round-robin\");", "Switches to round-robin."),
        answer("set_policy(\"round-robin\");", "Switches to round-robin."),
    ]);
    let reply = shell.eval("? switch to round robin");
    assert!(reply.text.starts_with("script 1: CHANGES set_policy\n"));
    assert_eq!(shell.eval("answer 1 no"), ok("script 1 not run"));
    assert_eq!(host.policy.get(), None);
    shell.eval("? switch to round robin");
    assert_eq!(shell.eval("answer 2 "), ok("script 2 not run"));
    assert_eq!(host.policy.get(), None);
    let audit = host.audit.borrow();
    assert!(audit.iter().any(|line| line == "script 1 declined"));
}

#[test]
fn a_yes_runs_it_and_a_stale_or_wrong_id_does_not() {
    let (host, mut shell) = with_answers(vec![answer("set_policy(\"round-robin\");", "Switches.")]);
    shell.eval("? switch to round robin");
    assert_eq!(shell.eval("answer 7 yes"), Reply::err("nothing to confirm"));
    assert_eq!(host.policy.get(), None);
    assert_eq!(shell.eval("answer 1 Yes"), ok(""));
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
}

#[test]
fn another_line_or_time_discards_the_proposal() {
    let (host, mut shell) = with_answers(vec![
        answer("print(1);", "Prints 1."),
        answer("print(2);", "Prints 2."),
    ]);
    shell.eval("? print one please");
    assert_eq!(shell.eval("1 + 1"), ok("2"));
    assert_eq!(shell.eval("answer 1 yes"), Reply::err("nothing to confirm"));
    shell.eval("? print two please");
    host.clock.set(host.clock.get() + 200_000_000_000);
    assert_eq!(
        shell.eval("answer 2 yes"),
        Reply::err("the script expired; ask again")
    );
}

#[test]
fn a_refused_script_is_repaired_once_with_the_reason() {
    let (host, mut shell) = with_answers(vec![
        answer("print(memory_free());", "Prints free memory."),
        answer("print(memory().free_mib);", "Prints free memory."),
    ]);
    let reply = shell.eval("? how much memory is free");
    assert_eq!(reply.confirm, Some(1), "{reply:?}");
    let requests = host.script_requests.borrow();
    assert_eq!(requests.len(), 2);
    let (script, error) = requests[1].repair.clone().unwrap();
    assert_eq!(script, "print(memory_free());");
    assert!(error.contains("memory_free"), "{error}");
    drop(requests);
    assert_eq!(shell.eval("answer 1 yes"), ok("100\n"));
}

#[test]
fn a_second_refusal_gives_up_and_nothing_is_proposed() {
    let (host, mut shell) = with_answers(vec![
        answer("loop { }", "Spins."),
        answer("loop { }", "Spins."),
        answer("print(1);", "never asked for"),
    ]);
    let reply = shell.eval("? spin forever please");
    assert!(!reply.ok && reply.confirm.is_none());
    assert!(
        reply
            .text
            .starts_with("gave up after one repair: syntax error"),
        "{reply:?}"
    );
    assert_eq!(host.script_requests.borrow().len(), 2);
    assert_eq!(shell.eval("answer 1 yes"), Reply::err("nothing to confirm"));
}

#[test]
fn a_cut_off_answer_is_not_repaired() {
    let (host, mut shell) = with_answers(vec![Err(AnswerError::CutOff)]);
    let reply = shell.eval("? write a long script");
    assert_eq!(
        reply,
        Reply::err("the answer was cut off at the token limit")
    );
    assert_eq!(host.script_requests.borrow().len(), 1);
}

#[test]
fn the_strict_profile_refuses_every_loop_closure_and_indirect_call() {
    for bad in [
        "loop { }",
        "while true { }",
        "do { } until false;",
        "for i in 0..3 { }",
        "let f = |x| x; f(1)",
        "fn f() { 1 } f()",
        "Fn(\"cpus\")",
        "\"cpus\".call()",
        "eval(\"cpus()\")",
        "print(undefined_variable);",
        "print(no_such_function());",
        "set_policy(\"fastest\");",
    ] {
        let result = check(
            &Answer {
                summary: String::new(),
                script: String::from(bad),
            },
            &DEFAULT_CONFIG,
        );
        assert!(result.is_err(), "{bad} was accepted");
    }
}

#[test]
fn a_trial_error_from_made_up_values_does_not_refuse() {
    // The stub lists four services and two CPUs; the real system may differ.
    for script in [
        "print(processes()[9].name);",
        "print(10 / (cpus() - 2));",
        "print(get_policy()[40]);",
    ] {
        let answer = Answer {
            summary: String::new(),
            script: String::from(script),
        };
        assert!(
            check(&answer, &DEFAULT_CONFIG).is_ok(),
            "{script} was refused"
        );
    }
    let (_host, mut shell) = with_answers(vec![answer("print(10 / (cpus() - 2));", "Divides.")]);
    shell.eval("? divide ten by the spare cpus");
    assert_eq!(shell.eval("answer 1 yes"), ok("5\n"));
    let (_, mut shell) = with_answers(vec![answer("print(processes()[9].name);", "Ninth.")]);
    shell.eval("? name the tenth service");
    assert!(!shell.eval("answer 1 yes").ok);
}

#[test]
fn the_capability_list_over_approximates() {
    assert_eq!(
        names_in("print(`${set_policy(\"rr\")}`);").1,
        ["set_policy"]
    );
    assert_eq!(names_in("\"rr\".set_policy();").1, ["set_policy"]);
    assert_eq!(
        names_in("// memory() is not called\nprint(1);").0,
        ["memory"]
    );
    assert_eq!(names_in("let set_policy_x = 1;").1, Vec::<&str>::new());
}

#[test]
fn a_full_size_script_still_fits_the_confirmation_limits() {
    // The most lines, each as long as the byte limit then allows.
    let per_line = llmproto::script::SCRIPT_MAX_BYTES / llmproto::script::SCRIPT_MAX_LINES - 1;
    let line = format!("print(\"{}\");", "x".repeat(per_line - 10));
    let script = std::iter::repeat_n(line.as_str(), llmproto::script::SCRIPT_MAX_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    let checked = check(
        &Answer {
            summary: "s".repeat(160),
            script,
        },
        &DEFAULT_CONFIG,
    )
    .unwrap();
    let text = crate::script::confirmation(99, &checked);
    assert!(text.len() + 20 < kabi::MAX_MESSAGE, "{} bytes", text.len());
    assert!(text.lines().count() <= 64);
    assert!(text.ends_with("run it? yes/no"));
}

#[test]
fn plain_english_goes_to_the_llm_and_near_miss_code_does_not() {
    assert!(reads_like_english("how much memory is free"));
    assert!(reads_like_english(
        "switch the scheduler to round robin please"
    ));
    assert!(!reads_like_english("let x be 5"));
    assert!(!reads_like_english("print hello world"));
    assert!(!reads_like_english("x + y + z"));
    assert!(!reads_like_english("mem"));
    let (host, mut shell) = with_answers(vec![answer("print(1);", "Prints 1.")]);
    assert_eq!(shell.eval("show me a number please").confirm, Some(1));
    assert!(shell.eval("let x be 5").text.starts_with("syntax error"));
    assert_eq!(host.script_requests.borrow().len(), 1);
}
