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
    };
    let decoded = apart::decode(&apart::encode(&ran)).unwrap();
    assert_eq!(
        (decoded.ok, &decoded.text, &decoded.done),
        (ran.ok, &ran.text, &ran.done)
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
