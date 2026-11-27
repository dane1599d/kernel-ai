//! Scripts run in a `scriptrun` child (P1 task 14): the wire both ways, the
//! child's own check, and what a child stopped for time leaves behind.

use super::scripts::{ran, with_answers};
use super::*;
use crate::apart;
use crate::script::Ran;
use crate::undo::{Done, Inverse};
use alloc::vec;

fn answer(script: &str) -> Result<Answer, AnswerError> {
    Ok(Answer {
        summary: String::from("Does it."),
        script: String::from(script),
    })
}

/// A child that runs each request with `host`, as `scriptrun` does with the
/// handles it was given.
fn child_of(host: &Rc<FakeHost>) {
    let dyn_host: Rc<dyn Host> = host.clone();
    *host.apart.borrow_mut() = Some(alloc::boxed::Box::new(move |request: &str| {
        Ok(apart::serve(request, &dyn_host, &DEFAULT_CONFIG))
    }));
}

#[test]
fn a_result_survives_the_wire_and_its_text_cannot_forge_a_change() {
    let ran = Ran {
        ok: true,
        text: String::from("line one\nset_policy kernel policy none\n3 abc\nran 1 ok=1 9 9"),
        done: vec![Done {
            action: "set_policy".into(),
            owner: "kernel".into(),
            res: "policy".into(),
            before: "interactive".into(),
            after: "round-robin".into(),
            inverse: Some(Inverse::Spec {
                policy: policy::INTERACTIVE,
                preset: 1,
                after: (policy::ROUND_ROBIN, 1),
            }),
        }],
        value: None,
        shown: Vec::new(),
        failed: None,
        withheld: false,
        unanswered: Some(String::from("notes_add")),
    };
    let decoded = apart::decode(&apart::encode(&ran)).unwrap();
    assert_eq!(
        (
            decoded.ok,
            &decoded.text,
            &decoded.done,
            &decoded.unanswered
        ),
        (ran.ok, &ran.text, &ran.done, &ran.unanswered)
    );
    for bad in [
        "",
        "ran 1 ok=2 0 0\n\n",
        "ran 1 ok=1 1 0\n\n",
        "ran 1 ok=1 0 99\nshort\n",
    ] {
        assert!(apart::decode(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn a_script_runs_in_the_child_and_its_change_is_logged_and_undone_as_before() {
    let (host, mut shell) = with_answers(vec![answer(
        "print(\"CPUs: \" + cpus());\nset_policy(\"rr\");",
    )]);
    child_of(&host);
    let reply = shell.eval("? how many cpus are there, then use round robin");
    assert_eq!(
        ran(&reply),
        "CPUs: 4\nDone: policy round-robin (was interactive). undo"
    );
    assert_eq!(host.apart_requests.borrow().len(), 1);
    assert!(host.apart_requests.borrow()[0].starts_with("script 1 cpus set_policy\n"));
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    assert_eq!(shell.eval("undo"), ok("Undone: policy back to interactive"));
}

#[test]
fn the_child_refuses_a_script_whose_calls_were_not_granted() {
    let host = FakeHost::new();
    let dyn_host: Rc<dyn Host> = host.clone();
    let answer = apart::serve(
        "script 1 cpus\nset_policy(\"rr\");",
        &dyn_host,
        &DEFAULT_CONFIG,
    );
    let ran = apart::decode(&answer).unwrap();
    assert!(
        !ran.ok && ran.text.contains("set_policy was not granted"),
        "{}",
        ran.text
    );
    assert_eq!(host.policy.get(), None);
}

#[test]
fn nothing_from_one_claim_reaches_the_next() {
    let host = FakeHost::new();
    let dyn_host: Rc<dyn Host> = host.clone();
    let first = apart::decode(&apart::serve(
        "script 1 set_policy\nlet kept = 5;\nset_policy(\"rr\");",
        &dyn_host,
        &DEFAULT_CONFIG,
    ))
    .unwrap();
    assert!(first.ok && first.done.len() == 1, "{}", first.text);
    // A fresh scope: the first claim's variable is not there, and the journal
    // starts empty.
    let second = apart::decode(&apart::serve(
        "script 1 \nprint(kept);",
        &dyn_host,
        &DEFAULT_CONFIG,
    ))
    .unwrap();
    assert!(!second.ok && second.done.is_empty(), "{}", second.text);
}

#[test]
fn a_child_stopped_for_time_leaves_nothing_changed() {
    let (host, mut shell) = with_answers(vec![answer("set_policy(\"rr\");\nprint(\"spins\");")]);
    // The child switched, then was stopped before it could answer.
    let switcher = host.clone();
    *host.apart.borrow_mut() = Some(alloc::boxed::Box::new(move |_: &str| {
        switcher.policy.set(Some(policy::ROUND_ROBIN));
        Err(String::from("stopped: the script ran past its time limit"))
    }));
    let reply = shell.eval("? use round robin and spin");
    assert!(
        reply
            .text
            .starts_with("Nothing changed: stopped: the script ran past its time limit"),
        "{reply:?}"
    );
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn ask_tier_rows_stay_in_the_shell_until_part_b() {
    let host = FakeHost::new();
    let dyn_host: Rc<dyn Host> = host.clone();
    let call = Answer {
        summary: String::new(),
        script: String::from("process_stop(\"build-3\");"),
    };
    let checked = crate::script::check_call(&call, &dyn_host, &DEFAULT_CONFIG).unwrap();
    assert!(!apart::may_run_apart(&checked));
    let read = super::scripts::checks("print(cpus());").unwrap();
    assert!(apart::may_run_apart(&read));
}

#[test]
fn window_rows_stay_in_the_shell_which_alone_reaches_desktop() {
    let host = FakeHost::new();
    let dyn_host: Rc<dyn Host> = host.clone();
    for script in [
        "ui_open(\"notes\");",
        "ui_focus(\"notes\");",
        "show(ui_view(\"notes\"));",
        "print(ui_windows().len());",
    ] {
        let call = Answer {
            summary: String::new(),
            script: String::from(script),
        };
        let checked =
            crate::script::check_against(&call, &dyn_host, &DEFAULT_CONFIG, &agentabi::UI_ROWS)
                .unwrap();
        assert!(!apart::may_run_apart(&checked), "{script}");
    }
}

#[test]
fn network_reads_stay_in_the_shell_which_alone_reaches_fetch() {
    let host = FakeHost::new();
    let dyn_host: Rc<dyn Host> = host.clone();
    for script in [
        "show(net_fetch(\"en.wikipedia.org\"));",
        "show(net_lookup(\"en.wikipedia.org\"));",
    ] {
        let call = Answer {
            summary: String::new(),
            script: String::from(script),
        };
        let checked = crate::script::check_against(
            &call,
            &dyn_host,
            &DEFAULT_CONFIG,
            &agentabi::SCRIPT_CALLS,
        )
        .unwrap();
        assert!(!apart::may_run_apart(&checked), "{script}");
    }
}

/// A child killed for time or Ctrl-C sends no journal back, so
/// `apart::killed` reads the policy, the one thing a child can change: every
/// other row a script may call only reads, or is a file row, which stays in
/// the shell. A new script row that changes state breaks this, and must give
/// `killed` a way to see that change, or stay in the shell.
#[test]
fn set_policy_is_the_one_change_a_killed_child_can_leave() {
    let changing: Vec<&str> = agentabi::SCRIPT_CALLS
        .iter()
        .filter(|row| row.inverse != agentabi::Inverse::Read && !row.name.starts_with("fs_"))
        .map(|row| row.name)
        .collect();
    assert_eq!(changing, ["set_policy"]);
}

#[test]
fn a_run_that_succeeded_after_an_unanswered_call_is_an_unknown_outcome_and_has_no_undo() {
    // The child says the run went on (ok=1) after a row's call went
    // unanswered, as a caught error would: the shell logs Unknown, says the
    // outcome is unknown, never Done, and a bare `undo` stops there rather
    // than reaching an older entry (#1164's review).
    let (host, mut shell) = with_answers(vec![answer("set_policy(\"rr\");")]);
    child_of(&host);
    assert!(shell.eval("? use round robin").ok);
    *host.apart.borrow_mut() = Some(alloc::boxed::Box::new(|_: &str| {
        Ok(String::from(
            "ran 1 ok=1 0 10 unanswered=notes_add\ncarried on\n",
        ))
    }));
    host.answers
        .borrow_mut()
        .push_back(answer("print(cpus());"));
    let reply = shell.eval("? how many cpus");
    assert!(
        reply
            .text
            .contains("Outcome unknown: notes_add may have run"),
        "{reply:?}"
    );
    assert!(!reply.text.contains("Done"), "{reply:?}");
    let entry = shell.log.get(shell.log.last_id()).expect("an entry");
    assert_eq!(entry.state, crate::actlog::State::Unknown);
    let undo = shell.eval("undo");
    assert!(!undo.ok, "{undo:?}");
    assert!(
        undo.text.contains("the outcome of notes_add is not known"),
        "{undo:?}"
    );
    assert_ne!(
        host.policy.get(),
        Some(policy::INTERACTIVE),
        "the older entry was not undone"
    );
}

#[test]
fn a_partial_rollback_after_an_unanswered_call_is_still_an_unknown_outcome() {
    // The run failed, one change could not be put back (no inverse), and a
    // row's call went unanswered: the outcome is unknown whatever else
    // happened, and a bare undo stops there (#1200's review).
    let ran = Ran {
        ok: false,
        text: String::from("it failed"),
        done: vec![Done {
            action: "set_policy".into(),
            owner: "kernel".into(),
            res: "policy".into(),
            before: "interactive".into(),
            after: "round-robin".into(),
            inverse: None,
        }],
        value: None,
        shown: Vec::new(),
        failed: None,
        withheld: false,
        unanswered: Some(String::from("notes_add")),
    };
    let wire = apart::encode(&ran);
    let (host, mut shell) = with_answers(vec![answer("print(cpus());")]);
    *host.apart.borrow_mut() = Some(alloc::boxed::Box::new(move |_: &str| Ok(wire.clone())));
    let reply = shell.eval("? how many cpus");
    assert!(!reply.ok, "{reply:?}");
    assert!(
        reply
            .text
            .contains("Outcome unknown: notes_add may have run"),
        "{reply:?}"
    );
    let entry = shell.log.get(shell.log.last_id()).expect("an entry");
    assert_eq!(entry.state, crate::actlog::State::Unknown);
    let undo = shell.eval("undo");
    assert!(
        undo.text.contains("the outcome of notes_add is not known"),
        "{undo:?}"
    );
}

#[test]
fn an_unknown_outcomes_known_changes_are_undone_by_number_never_by_a_bare_undo() {
    // #1200's review: an entry logged Unknown keeps the changes that are
    // known; a bare undo stops at it, and `undo <id>` puts those back.
    let (host, mut shell) = with_answers(vec![answer("set_policy(\"rr\");")]);
    child_of(&host);
    assert!(shell.eval("? use round robin").ok);
    let first = shell.log.last_id();
    let mut unknown = shell.log.get(first).expect("the run's entry").clone();
    // Its own id, as the shell gives one: the next record must not take it too.
    let id = shell.next_id;
    shell.next_id += 1;
    unknown.id = id;
    unknown.state = crate::actlog::State::Unknown;
    unknown.outcome = String::from("failed; notes_add may have run");
    shell.log.push(unknown);
    let bare = shell.eval("undo");
    assert!(
        bare.text.contains("the outcome of notes_add is not known"),
        "{bare:?}"
    );
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    let by_id = shell.eval(&format!("undo {id}"));
    assert!(by_id.ok, "{by_id:?}");
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}
