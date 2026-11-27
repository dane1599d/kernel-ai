//! The act rows past the plain ones: stopping and starting services, the
//! rules an Always allow writes and the ONCEs that go to their owners;
//! what Kai can do with something, from the rows; and the lines after a
//! clarifying question.

use super::*;

const STOP_BUILD3: &str = r#"{"name":"process_stop","arguments":{"name":"build-3"}}"#;

#[test]
fn a_stop_asks_once_with_rules_and_an_always_allow_waives_the_next() {
    let (host, mut shell) = with_calls(vec![call(STOP_BUILD3), call(STOP_BUILD3)]);
    host.procs.borrow_mut().push(String::from("build-3"));
    let reply = shell.eval("stop build-3");
    // Plainly asked, and no undo: the highlight starts on Allow, and both rules
    // are offered.
    let question = "Run process_stop(\"build-3\")? It has no undo.";
    let call = "process_stop(\"build-3\")";
    assert_eq!(
        reply.to_wire(),
        format!(
            "menu 1 asked\n{question}\nprocess_stop calls\nprocess_stop on \"build-3\"\n{call}"
        )
    );
    let once = host.mint(1, call);
    let reply = shell.eval(&format!("answer 1 always-target once={once} rule=5"));
    assert_eq!(
        reply,
        ok("Rule #1 added; `rules` lists it.\nDone: process build-3 stopped (2) (was running)")
    );
    assert_eq!(shell.eval("rules"), ok("#1 process_stop \"build-3\""));
    // The rule waives the next ask for that target, and the log says so.
    host.procs.borrow_mut().push(String::from("build-3"));
    let reply = shell.eval("stop build-3");
    assert_eq!(reply, ok("Done: process build-3 stopped (1) (was running)"));
    let actlog = shell.eval("log actlog").text;
    assert!(actlog.contains("consent=rule:1"), "{actlog}");
    // Removing it revokes its grant node and the next one asks again.
    assert_eq!(
        shell.eval("rules_remove 1"),
        ok("Removed rule #1 (process_stop); it asks again")
    );
    assert_eq!(*host.revoked.borrow(), [5]);
}

/// bug-hunt-6 H6-17: a rule's `made=` is the wall clock, not the boot clock.
#[test]
fn a_saved_rule_is_stamped_with_the_wall_clock() {
    let (host, mut shell) = with_calls(vec![call(STOP_BUILD3)]);
    host.wall_clock.set(Some(1_794_637_800));
    host.clock.set(312_000_000_000);
    host.procs.borrow_mut().push(String::from("build-3"));
    shell.eval("stop build-3");
    let once = host.mint(1, "process_stop(\"build-3\")");
    shell.eval(&format!("answer 1 always-target once={once} rule=5"));
    let disk = host.rules_disk.borrow().clone().unwrap();
    assert!(disk.contains("made=2026-11-14T06:30:00Z"), "{disk}");
}

#[test]
fn a_rule_survives_a_restart_as_a_record_and_asks_until_allowed_again() {
    let (host, mut shell) = with_calls(vec![call(STOP_BUILD3), call(STOP_BUILD3)]);
    host.procs.borrow_mut().push(String::from("build-3"));
    shell.eval("stop build-3");
    let once = host.mint(1, "process_stop(\"build-3\")");
    shell.eval(&format!("answer 1 always-target once={once} rule=5"));
    let disk = host.rules_disk.borrow().clone().unwrap();
    assert!(
        disk.starts_with(
            "kai-grants 1
rule 1 action=process_stop target=build-3 from=prompt:1 made="
        ),
        "{disk}"
    );
    // A new shell reads it back with no node, so the stop's owner would refuse
    // it: the stop asks again until it is allowed on this boot.
    let mut shell = super::shell(&host);
    assert_eq!(
        shell.eval("rules"),
        ok("#1 process_stop \"build-3\" (asks again until allowed on this boot)")
    );
    host.procs.borrow_mut().push(String::from("build-3"));
    assert!(shell.eval("stop build-3").to_wire().starts_with("menu "));
    assert_eq!(
        shell.eval("rules_remove 1"),
        ok("Removed rule #1 (process_stop); it asks again")
    );
    assert_eq!(
        host.rules_disk.borrow().as_deref(),
        Some(
            "kai-grants 1
"
        )
    );
}

#[test]
fn a_rule_for_one_target_does_not_cover_another_and_protected_names_never_stop() {
    let stop4 = r#"{"name":"process_stop","arguments":{"name":"build-4"}}"#;
    let llm = r#"{"name":"service_stop","arguments":{"name":"llm"}}"#;
    let (host, mut shell) = with_calls(vec![call(STOP_BUILD3), call(stop4), call(llm)]);
    shell.eval("stop build-3");
    let once = host.mint(1, "process_stop(\"build-3\")");
    shell.eval(&format!("answer 1 always-target once={once}"));
    assert!(
        shell
            .eval("stop build-4")
            .text
            .starts_with("menu 2 asked\n")
    );
    shell.eval("n");
    let reply = shell.eval("stop the llm service");
    assert!(!reply.ok && reply.text.contains("never"), "{reply:?}");
}

#[test]
fn a_start_runs_at_once_and_undo_stops_it_again() {
    let start = r#"{"name":"service_start","arguments":{"name":"notes"}}"#;
    let (host, mut shell) = with_calls(vec![call(start)]);
    let reply = shell.eval("start notes");
    assert_eq!(reply, ok("Done: service notes running (was stopped). undo"));
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: service notes stopped again")
    );
    assert!(!host.services.borrow()[0].1);
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: service notes started again")
    );
}

/// A shell whose host lists its services: `notes` stopped, `indexer` running,
/// and plain processes `build-3` and `build-4`.
fn with_services(calls: Vec<Result<Call, ActError>>) -> (Rc<FakeHost>, Shell) {
    let (host, shell) = with_calls(calls);
    host.services
        .borrow_mut()
        .push((String::from("indexer"), true));
    host.lists_services.set(true);
    (host, shell)
}

#[test]
fn a_stop_runs_the_row_for_what_the_name_is_and_the_menu_names_it() {
    // The model picked the service row for a plain process, and the process row
    // for a service: code looks the names up.
    let wrong_process = r#"{"name":"service_stop","arguments":{"name":"build-3"}}"#;
    let wrong_service = r#"{"name":"process_stop","arguments":{"name":"indexer"}}"#;
    let (host, mut shell) = with_services(vec![call(wrong_process), call(wrong_service)]);
    let reply = shell.eval("stop build-3");
    let call = "process_stop(\"build-3\")";
    assert_eq!(
        reply.to_wire(),
        format!(
            "menu 1 asked\nStop process build-3 (pid 30) with {call}? It has no undo.\nprocess_stop calls\nprocess_stop on \"build-3\"\n{call}"
        )
    );
    let once = host.mint(1, call);
    let reply = shell.eval(&format!("answer 1 allow once={once}"));
    assert!(
        reply.ok && reply.text.contains("build-3 stopped"),
        "{reply:?}"
    );
    let audit = host.audit.borrow().join("\n");
    assert!(
        audit.contains("service_stop on build-3 runs as process_stop"),
        "{audit}"
    );
    let reply = shell.eval("kill indexer").to_wire();
    assert!(
        reply.starts_with("menu 2 ")
            && reply.contains("\nStop service indexer with service_stop(\"indexer\")? ")
            && reply.ends_with("\nservice_stop(\"indexer\")"),
        "{reply}"
    );
}

#[test]
fn starting_a_stopped_service_again_starts_it() {
    let restart = r#"{"name":"service_restart","arguments":{"name":"notes"}}"#;
    let (host, mut shell) = with_services(vec![call(restart)]);
    let reply = shell.eval("start notes again");
    assert_eq!(reply, ok("Done: service notes running (was stopped). undo"));
    assert!(host.services.borrow()[0].1);
}

#[test]
fn a_name_that_is_both_or_neither_asks_which() {
    let both = r#"{"name":"process_stop","arguments":{"name":"indexer"}}"#;
    let neither = r#"{"name":"process_stop","arguments":{"name":"build-9"}}"#;
    let restart = r#"{"name":"service_restart","arguments":{"name":"build-3"}}"#;
    let (host, mut shell) = with_services(vec![call(both), call(neither), call(restart)]);
    host.procs.borrow_mut().push(String::from("indexer"));
    let reply = shell.eval("stop indexer");
    assert_eq!(
        reply,
        ok("indexer is both a service and a separate process (pid 32). Which one?")
    );
    // A request of its own leaves the question, with a note (clarify::answers).
    let reply = shell.eval("stop build-9");
    assert_eq!(
        reply,
        ok(
            "(Left the question about \"stop indexer\".)\nNo service or process named build-9 is running. Which one did you mean?"
        )
    );
    let reply = shell.eval("restart build-3");
    assert!(
        reply.ok && reply.text.contains("not a service"),
        "{reply:?}"
    );
    assert_eq!(host.procs.borrow().len(), 3, "nothing was stopped");
}

#[test]
fn what_is_already_so_is_said_and_nothing_runs() {
    let stop = r#"{"name":"service_stop","arguments":{"name":"notes"}}"#;
    let start = r#"{"name":"service_start","arguments":{"name":"indexer"}}"#;
    let (host, mut shell) = with_services(vec![call(stop), call(start)]);
    assert_eq!(
        shell.eval("stop notes"),
        ok("Service notes is already stopped.")
    );
    assert_eq!(
        shell.eval("start the indexer"),
        ok("Service indexer is already running.")
    );
    assert!(!host.services.borrow()[0].1 && host.services.borrow()[1].1);
}

#[test]
fn the_model_cannot_list_or_remove_standing_rules() {
    let remove = r#"{"name":"rules_remove","arguments":{"n":1}}"#;
    let list = r#"{"name":"rules","arguments":{}}"#;
    let (host, mut shell) = with_calls(vec![call(STOP_BUILD3), call(remove), call(list)]);
    shell.eval("stop build-3");
    let once = host.mint(1, "process_stop(\"build-3\")");
    shell.eval(&format!("answer 1 always-kind once={once}"));
    // "remove ..." is a package line (`pkg::line`), so the model is asked with "delete".
    for line in ["delete my first rule", "what are my rules"] {
        let reply = shell.eval(line);
        assert_eq!(
            reply,
            Reply::err(format!(
                "Refused {}: no such action",
                if line.starts_with("delete") {
                    "rules_remove"
                } else {
                    "rules"
                }
            )),
            "{line}"
        );
    }
    assert_eq!(shell.eval("rules"), ok("#1 process_stop"));
}

#[test]
fn a_script_cannot_call_a_call_only_row() {
    // The script prompt does not list them, and a script that calls one anyway
    // fails its trial: a stop asks once, which a script's consent cannot cover.
    let script = r#"{"name":"script","arguments":{"code":"service_stop(\"notes\");"}}"#;
    let (host, mut shell) = with_calls(vec![call(script)]);
    let reply = shell.eval("stop the notes service");
    assert!(!reply.ok, "{reply:?}");
    assert!(!llmproto::script::system_prompt().contains("service_stop"));
    assert!(host.services.borrow().iter().all(|(_, running)| !running));
}

/// P1 task 14: a stop's owner, not the shell, checks and spends its proof.
#[test]
fn a_stops_once_goes_to_its_owner_unspent_and_a_rule_or_nothing_is_said_so() {
    let (host, mut shell) = with_calls(vec![
        call(STOP_BUILD3),
        call(STOP_BUILD3),
        call(STOP_BUILD3),
    ]);
    host.procs.borrow_mut().push(String::from("build-3"));
    shell.eval("stop build-3");
    let once = host.mint(1, "process_stop(\"build-3\")");
    let reply = shell.eval(&format!("answer 1 always-target once={once} rule=5"));
    assert!(
        reply
            .text
            .ends_with("Done: process build-3 stopped (2) (was running)"),
        "{reply:?}"
    );
    // The owner got the ONCE unspent and spent it; the shell did not check it.
    assert_eq!(
        host.proofs.borrow()[0],
        (
            String::from("process_stop(\"build-3\")"),
            Proof::Once { badge: once, id: 1 }
        )
    );
    assert!(host.once.borrow()[0].2, "the owner's check used it up");
    // The next stop runs under the rule, and says so to the owner.
    host.procs.borrow_mut().push(String::from("build-3"));
    assert!(shell.eval("stop build-3").ok);
    assert_eq!(host.proofs.borrow()[1].1, Proof::Rule);
    // With the rule gone, a reused ONCE is refused by the owner.
    shell.eval("rules_remove 1");
    host.procs.borrow_mut().push(String::from("build-3"));
    shell.eval("stop build-3");
    let reply = shell.eval(&format!("answer 3 allow once={once}"));
    assert!(!reply.text.contains("Done:"), "{reply:?}");
    assert!(host.procs.borrow().contains(&String::from("build-3")));
}

#[test]
fn a_restart_is_one_request_with_its_once() {
    let restart = r#"{"name":"service_restart","arguments":{"name":"indexer"}}"#;
    let (host, mut shell) = with_services(vec![call(restart)]);
    shell.eval("restart indexer");
    let once = host.mint(1, "service_restart(\"indexer\")");
    let reply = shell.eval(&format!("answer 1 allow once={once}"));
    assert!(reply.ok && reply.text.contains("restarted"), "{reply:?}");
    assert_eq!(
        *host.proofs.borrow(),
        [(
            String::from("service_restart(\"indexer\")"),
            Proof::Once { badge: once, id: 1 }
        )]
    );
}

#[test]
fn undo_stops_only_the_start_its_own_entry_made() {
    let start = r#"{"name":"service_start","arguments":{"name":"notes"}}"#;
    let (host, mut shell) = with_services(vec![call(start), call(start)]);
    assert!(shell.eval("start notes").ok);
    // The start told the supervisor its entry; the undo shows that entry.
    assert_eq!(*host.started_by.borrow(), [(String::from("notes"), 1)]);
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: service notes stopped again")
    );
    assert_eq!(host.proofs.borrow()[0].1, Proof::Undo { id: 1 });
    // Its redo starts it for the undo's own entry, whose undo then stops it.
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: service notes started again")
    );
    assert_eq!(*host.started_by.borrow(), [(String::from("notes"), 3)]);
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: service notes stopped again")
    );
    assert_eq!(host.proofs.borrow()[1].1, Proof::Undo { id: 3 });
    // A start the supervisor recorded for another entry is not this undo's.
    assert!(shell.eval("start notes").ok);
    host.started_by.borrow_mut()[0].1 = 99;
    assert!(!shell.eval("undo").ok);
    assert!(host.services.borrow()[0].1, "notes still runs");
}

#[test]
fn what_kai_can_do_with_something_rows_name_comes_from_the_rows() {
    // The founder's VirtualBox test (2026-11-14): no invented "CPU frequency",
    // and no model asked when rows name the thing.
    let (host, mut shell) = with_calls(vec![]);
    let cpus = shell.eval("what can you do with the cpus");
    assert!(
        cpus.ok && cpus.text.contains("- set_policy: switches every CPU"),
        "{cpus:?}"
    );
    assert!(!cpus.text.contains("frequency"), "{cpus:?}");
    assert!(host.act_lines.borrow().is_empty());
}

#[test]
fn a_topic_no_row_names_or_one_meaning_everything_goes_to_the_model() {
    use crate::abilities::answer;
    // Whole words only: "me" is not "memory", and no match is no proof.
    for line in [
        "what can you do for me",
        "what can you do on this computer",
        "what can you do with the network",
        "can you ican you do anything with the gpu?",
        "can you use the gpu",
        "what can you do",
        "show me the cpus",
        "what can you do with the cpus and then switch the policy to round robin",
    ] {
        assert_eq!(answer(line), None, "{line}");
    }
    // The founder's own line, typo and all, finds its opening.
    assert!(answer("can you ican you do anything with the cpus?").is_some());
    assert!(answer("can you use the cpus").is_some());
}

#[test]
fn a_clarify_offers_only_what_some_row_backs() {
    let ask = r#"{"name":"clarify","arguments":{"question":"What can you do with CPUs?","options":["adjust scheduling policy","increase CPU frequency","use GPU for rendering"]}}"#;
    let (_host, mut shell) = with_calls(vec![call(ask)]);
    let reply = shell.eval("tell me about cpus and stuff");
    assert_eq!(
        reply,
        ok("What can you do with CPUs? (adjust scheduling policy)")
    );
}

#[test]
fn a_new_request_after_a_question_runs_alone() {
    // The founder's VirtualBox test (2026-11-14): a stray line, a question
    // back, then a request of its own, which must not join the stray line.
    let start = r#"{"name":"service_start","arguments":{"name":"sshd"}}"#;
    let cpus = r#"{"name":"script","arguments":{"code":"print(\"CPUs: \" + cpus());"}}"#;
    let (host, mut shell) = with_services(vec![call(start), call(cpus)]);
    assert_eq!(
        shell.eval("sssssshd"),
        ok("No service or process named sshd is running. Which one did you mean?")
    );
    let reply = shell.eval("show me the cpus");
    // A short note says the question was left, then the request's own answer.
    assert_eq!(
        reply,
        ok("(Left the question about \"sssssshd\".)\nCPUs: 4\n")
    );
    assert_eq!(host.act_lines.borrow()[1], "show me the cpus");
    let audit = host.audit.borrow();
    assert!(
        audit
            .iter()
            .any(|line| line.contains("reads as a new request")),
        "{audit:?}"
    );
}

#[test]
fn an_answer_to_the_question_still_continues_the_request() {
    let start = r#"{"name":"service_start","arguments":{"name":"sshd"}}"#;
    let netd = r#"{"name":"service_start","arguments":{"name":"indexer"}}"#;
    let (host, mut shell) = with_services(vec![call(start), call(netd)]);
    shell.eval("sssssshd");
    shell.eval("the indexer one");
    assert_eq!(host.act_lines.borrow()[1], "sssssshd the indexer one");
}

#[test]
fn only_a_line_that_opens_like_a_request_leaves_the_question() {
    use crate::clarify::answers;
    for line in [
        "sshd",
        "round robin",
        "the second one",
        "stop",
        "netd please",
        "indexer",
    ] {
        assert!(answers(line), "{line}");
    }
    for line in [
        "show me the cpus",
        "What is the policy?",
        "please list apps",
        "Stop netd",
        "can you do anything with the gpu",
        "Smake a note called ideas and type hello dane into it",
    ] {
        assert!(!answers(line), "{line}");
    }
}

#[test]
fn a_telling_request_and_its_answer_still_refuse_an_added_change() {
    // ADR 0002's telling-line rule judges the whole request: the person asked
    // to be told, and a short answer to the question back doesn't change that.
    let ask = r#"{"name":"clarify","arguments":{"question":"Which?","options":["interactive","round-robin"]}}"#;
    let (host, mut shell) = with_calls(vec![call(ask), call(RR)]);
    shell.eval("what is the policy");
    let reply = shell.eval("round robin");
    assert!(
        !reply.ok && reply.text.contains("only asks to be told"),
        "{reply:?}"
    );
    assert_eq!(host.act_lines.borrow()[1], "what is the policy round robin");
}

#[test]
fn a_long_line_after_a_question_is_a_request_of_its_own() {
    let ask = r#"{"name":"clarify","arguments":{"question":"Which?","options":["a","b"]}}"#;
    let (host, mut shell) = with_calls(vec![call(ask), call(RR)]);
    // Not "what can you do with the cpus": that is answered from the rows.
    shell.eval("tell me about the cpus");
    shell.eval("Smake a note called ideas and type hello dane into it");
    assert_eq!(
        host.act_lines.borrow()[1],
        "Smake a note called ideas and type hello dane into it"
    );
}

#[test]
fn what_time_it_is_comes_from_the_clock_not_the_model() {
    // Founder test 2, item 12: no invented time, and no change card.
    let (host, mut shell) = with_calls(vec![]);
    let reply = shell.eval("what time is it?");
    assert!(
        reply.ok && reply.text.starts_with("I can't tell the time"),
        "{reply:?}"
    );
    assert!(host.act_lines.borrow().is_empty());
}

#[test]
fn undo_agent_takes_back_that_agents_newest_entry_and_why_names_it() {
    let (host, mut shell) = with_calls(vec![call(RR)]);
    shell.eval("switch to round robin");
    let id = shell.log.last_id();
    // As if agent `goals` had made it (ADR 0013 §1).
    if let Some(entry) = shell.log.get_mut(id) {
        entry.agent = agentid::Name::new("goals").unwrap();
    }
    assert!(shell.eval("why").text.ends_with("\nby goals"));
    assert!(
        shell
            .eval("log")
            .text
            .contains(&format!("#{id} [goals] switch to round robin"))
    );
    assert!(shell.eval("undo goals").text.starts_with("Undone: "));
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
    assert_eq!(
        shell.eval("undo goals"),
        Reply::err("nothing of goals's to undo")
    );
}
