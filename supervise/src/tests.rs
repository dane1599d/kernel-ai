use super::*;
use alloc::vec;

#[test]
fn plan_splits_limits_from_program_arguments() {
    let plan = parse_plan(
        "llm caps=serve+net server=10.0.2.2:8080 model=mock\n\n  spinner burst_ms=300\nhog pages=64",
    )
    .services;
    assert_eq!(plan.len(), 3);
    assert_eq!(plan[0].name, "llm");
    assert_eq!(plan[0].args, "server=10.0.2.2:8080 model=mock");
    assert_eq!(plan[0].limits.caps, cap::SERVE | cap::NET);
    assert_eq!(plan[1].limits.max_burst_ms, 300);
    assert_eq!(plan[1].limits.max_pages, DEFAULT_PAGES);
    assert_eq!(plan[2].limits.max_pages, 64);
    assert_eq!(plan[2].limits.caps, 0);
    assert_eq!(plan[2].limits.stack_pages, 0);
    assert_eq!(plan[2].policy.max_restarts, POLICY.max_restarts);
}

#[test]
fn the_local_llm_line_names_the_model_capability() {
    let plan = parse_plan("llm caps=serve+net+model burst_ms=10000 pages=1331").services;
    assert_eq!(plan[0].limits.caps, cap::SERVE | cap::NET | cap::MODEL);
    assert_eq!(plan[0].limits.max_burst_ms, 10_000);
    assert_eq!(plan[0].limits.max_pages, 1331);
    assert_eq!(plan[0].args, "");
    let plan = parse_plan("llm caps=serve+model+threads").services;
    assert_eq!(plan[0].limits.caps, cap::SERVE | cap::MODEL | cap::THREADS);
    let plan = parse_plan("netcheck caps=resolve").services;
    assert_eq!(plan[0].limits.caps, cap::RESOLVE);
    assert_ne!(cap::GRANTABLE & cap::RESOLVE, 0);
}

#[test]
fn plan_can_tune_the_restart_policy_per_service() {
    let plan = parse_plan("crasher backoff_max_ms=200 max_restarts=1000 after_ms=150").services;
    assert_eq!(plan[0].policy.max_backoff_ms, 200);
    assert_eq!(plan[0].policy.max_restarts, 1000);
    assert_eq!(plan[0].policy.first_backoff_ms, POLICY.first_backoff_ms);
    assert_eq!(plan[0].args, "after_ms=150");
}

#[test]
fn the_plan_the_kernel_sends_parses() {
    let text = alloc::format!(
        "llm caps=serve+net burst_ms=10000 pages=1024 host=10.0.2.2:8080 model=org/model-7b\n{}",
        kabi::plan::SHELL
    );
    let plan = parse_plan(&text);
    assert_eq!(plan.errors, Vec::<String>::new());
    let shell = &plan.services[1];
    assert_eq!(shell.name, "shell");
    assert_eq!(
        shell.limits.caps,
        cap::SERVE | cap::CALL | cap::POLICY | cap::STOP | cap::SCRIPT
    );
    assert_eq!(shell.uses, ["llm", SUPERVISOR]);
    assert_eq!(shell.limits.stack_pages, 512);
    assert!(shell.limits.stack_pages < shell.limits.max_pages);
    assert_eq!(
        plan.services[0].args,
        "host=10.0.2.2:8080 model=org/model-7b"
    );
}

#[test]
fn a_bad_line_skips_only_its_service() {
    let plan = parse_plan("llm caps=serve+net\nshell caps=serve+teleport\nhog pages=64");
    let names: Vec<&str> = plan.services.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["llm", "hog"]);
    assert_eq!(plan.errors.len(), 1);
    assert!(plan.errors[0].contains("teleport"));
}

#[test]
fn plan_rejects_bad_lines() {
    for bad in [
        "a caps=supervise",
        "a pages=0",
        "a pages=x",
        "a stack_pages=4096",
        "bad/name",
        "a-very-long-service-name-indeed",
    ] {
        let plan = parse_plan(bad);
        assert!(plan.services.is_empty() && plan.errors.len() == 1, "{bad}");
    }
    assert_eq!(parse_plan("a\na").errors.len(), 1);
}

#[test]
fn quick_failures_back_off_exponentially_up_to_the_cap() {
    let mut record = Record::new("crasher");
    let delays: Vec<Decision> = (0..8)
        .map(|i| record.on_exit(&POLICY, i * 1000, 200))
        .collect();
    let expected = [50, 100, 200, 400, 800, 1600, 1600, 1600].map(Decision::RestartAfterMs);
    assert_eq!(delays, expected);
    assert_eq!(record.restarts, 8);
}

#[test]
fn a_healthy_run_resets_the_backoff() {
    let mut record = Record::new("llm");
    record.on_exit(&POLICY, 0, 10);
    record.on_exit(&POLICY, 100, 10);
    assert_eq!(
        record.on_exit(&POLICY, 20_000, 10_000),
        Decision::RestartAfterMs(0)
    );
    assert_eq!(
        record.on_exit(&POLICY, 20_100, 10),
        Decision::RestartAfterMs(50)
    );
}

#[test]
fn too_many_restarts_in_the_window_gives_up_for_good() {
    let policy = Policy {
        max_restarts: 3,
        ..POLICY
    };
    let mut record = Record::new("crasher");
    for i in 0..3 {
        assert!(matches!(
            record.on_exit(&policy, i, 1),
            Decision::RestartAfterMs(_)
        ));
    }
    assert_eq!(record.on_exit(&policy, 3, 1), Decision::GiveUp);
    assert_eq!(record.on_exit(&policy, 999_999, 1), Decision::GiveUp);
}

#[test]
fn old_restarts_leave_the_window() {
    let policy = Policy {
        max_restarts: 2,
        window_ms: 1000,
        ..POLICY
    };
    let mut record = Record::new("s");
    record.on_exit(&policy, 0, 1);
    record.on_exit(&policy, 10, 1);
    assert!(matches!(
        record.on_exit(&policy, 2000, 1),
        Decision::RestartAfterMs(_)
    ));
}

#[test]
fn state_round_trips_and_damage_is_ignored() {
    let records = vec![
        Record {
            name: "crasher".into(),
            restarts: 7,
            quick_failures: 3,
            recent: vec![10, 20, 30],
            gave_up: false,
            stopped: false,
            pid: 42,
            started_ms: 1234,
        },
        Record {
            name: "hog".into(),
            restarts: 1,
            quick_failures: 0,
            recent: vec![],
            gave_up: true,
            stopped: true,
            pid: 0,
            started_ms: 0,
        },
    ];
    assert_eq!(decode(encode(&records).as_bytes()), records);
    assert!(decode(b"").is_empty());
    assert!(decode(b"garbage").is_empty());
    assert!(decode(&[0xFF, 0xFE]).is_empty());
    let damaged = alloc::format!("{STATE_MAGIC}\ncrasher x 1 0 0 0 0\nhog 2 0 0 0 7 99");
    // State in an older layout reads as none rather than as shifted fields.
    assert!(decode(b"supervisor-state 2\nhog 2 0 0 7").is_empty());
    assert_eq!(decode(damaged.as_bytes()).len(), 1);
    // Version 3 had no stopped flag: its records carry over, none stopped.
    let old = decode(
        b"supervisor-state 3
hog 2 1 0 7 99 5",
    );
    assert_eq!(old.len(), 1);
    assert_eq!(
        (old[0].restarts, old[0].pid, old[0].recent.clone()),
        (2, 7, vec![5])
    );
    assert!(!old[0].stopped);
}

#[test]
fn stop_and_start_requests_parse_and_are_checked() {
    assert_eq!(
        Request::parse("stop build-1"),
        Some((Request::Stop("build-1"), Proof::None))
    );
    assert_eq!(
        Request::parse(" start  build-1 "),
        Some((Request::Start("build-1"), Proof::None))
    );
    let once = Once { badge: 9, id: 4 };
    for (text, want) in [
        (
            "restart build-1 once=9 id=4",
            (Request::Restart("build-1"), Proof::Once(once)),
        ),
        ("stop build-1 rule", (Request::Stop("build-1"), Proof::Rule)),
        (
            "suspend notes rule",
            (Request::Suspend("notes"), Proof::Rule),
        ),
        ("resume notes rule", (Request::Resume("notes"), Proof::Rule)),
        (
            "stop build-1 undo id=4",
            (Request::Stop("build-1"), Proof::Undo(4)),
        ),
        (
            "start build-1 id=4",
            (Request::Start("build-1"), Proof::Entry(4)),
        ),
    ] {
        assert_eq!(Request::parse(text), Some(want), "{text:?}");
    }
    for bad in [
        "",
        "stop",
        "stop a b",
        "STOP a",
        "stop a once=9",
        "stop a id=4 once=9",
        "stop a once=x id=4",
        "stop a once=9 id=4 more",
        "stop a undo",
        "stop a rule id=4",
    ] {
        assert_eq!(Request::parse(bad), None, "{bad:?}");
    }
    let mut records = vec![Record::new("llm"), Record::new("build-1")];
    assert_eq!(check(Request::Stop("llm"), &records), Err("err protected"));
    assert_eq!(
        check(Request::Stop("shell"), &records),
        Err("err protected")
    );
    assert_eq!(
        check(Request::Start("supervisor"), &records),
        Err("err protected")
    );
    assert_eq!(
        check(Request::Stop("nope"), &records),
        Err("err unknown service")
    );
    assert_eq!(
        check(Request::Start("build-1"), &records),
        Err("err not stopped")
    );
    assert_eq!(check(Request::Stop("build-1"), &records), Ok(1));
    records[1].stopped = true;
    assert_eq!(
        check(Request::Stop("build-1"), &records),
        Err("err already stopped")
    );
    assert_eq!(check(Request::Start("build-1"), &records), Ok(1));
    // A service the supervisor gave up on may be started.
    records[1].stopped = false;
    records[1].gave_up = true;
    assert_eq!(check(Request::Start("build-1"), &records), Ok(1));
    records[1].gave_up = false;
    records[1].stopped = true;
    // A restart starts a stopped service, and restarts a running one.
    assert_eq!(check(Request::Restart("build-1"), &records), Ok(1));
    records[1].stopped = false;
    assert_eq!(check(Request::Restart("build-1"), &records), Ok(1));
    assert_eq!(
        check(Request::Restart("llm"), &records),
        Err("err protected")
    );
}

#[test]
fn a_start_needs_no_proof_and_a_stop_or_restart_needs_one() {
    let once = Proof::Once(Once { badge: 1, id: 2 });
    let fits = |request, proof| proof_fits(request, proof).is_ok();
    assert!(fits(Request::Start("a"), Proof::None));
    assert!(fits(Request::Start("a"), Proof::Entry(3)));
    assert!(!fits(Request::Start("a"), once));
    for request in [Request::Stop("a"), Request::Restart("a")] {
        assert!(fits(request, once));
        assert!(fits(request, Proof::Rule));
        assert_eq!(proof_fits(request, Proof::None), Err("err needs ONCE"));
        assert_eq!(proof_fits(request, Proof::Entry(3)), Err("err needs ONCE"));
    }
    // Only a stop undoes a start.
    assert!(fits(Request::Stop("a"), Proof::Undo(3)));
    assert!(!fits(Request::Restart("a"), Proof::Undo(3)));
}

#[test]
fn a_request_digests_the_call_the_menu_showed() {
    let once = Once { badge: 77, id: 7 };
    for (request, call) in [
        (Request::Stop("indexer"), "service_stop(\"indexer\")"),
        (Request::Start("indexer"), "service_start(\"indexer\")"),
        (Request::Restart("indexer"), "service_restart(\"indexer\")"),
    ] {
        assert_eq!(request.call().as_deref(), Some(call));
        assert_eq!(
            request.digest(7),
            Some(termcore::once::digest_call(7, call))
        );
        // Another entry, verb or name digests differently.
        assert_ne!(request.digest(7), request.digest(8));
        for proof in [
            Proof::Once(once),
            Proof::Rule,
            Proof::Undo(3),
            Proof::Entry(3),
            Proof::None,
        ] {
            let text = request.encode(proof);
            assert_eq!(Request::parse(&text), Some((request, proof)), "{text:?}");
        }
    }
    assert_ne!(
        Request::Stop("indexer").digest(7),
        Request::Restart("indexer").digest(7)
    );
    assert_ne!(
        Request::Stop("indexer").digest(7),
        Request::Stop("indexes").digest(7)
    );
    // A process stop shown on the menu does not pass for a service stop.
    assert_ne!(
        Request::Stop("indexer").digest(7),
        Some(termcore::once::digest_call(7, "process_stop(\"indexer\")"))
    );
}

#[test]
fn services_lists_pids_and_stops_but_no_protected_name() {
    let mut records = vec![
        Record::new("llm"),
        Record::new("indexer"),
        Record::new("notes"),
    ];
    records[1].pid = 12;
    records[2].stopped = true;
    let text = listing(&records);
    assert_eq!(text, "ok indexer=12 notes=stopped");
    let listed = parse_listing(&text).unwrap();
    assert_eq!(
        listed,
        vec![
            Listed {
                name: "indexer".into(),
                pid: 12,
                stopped: false
            },
            Listed {
                name: "notes".into(),
                pid: 0,
                stopped: true
            },
        ]
    );
    assert_eq!(parse_listing("ok"), Some(vec![]));
    for bad in ["", "err unknown request", "ok indexer", "ok a=x"] {
        assert_eq!(parse_listing(bad), None, "{bad:?}");
    }
}

#[test]
fn unsupervised_lines_take_no_caps_and_stop_is_a_cap() {
    let plan = parse_plan(
        "spinner program=spinner unsupervised
shell caps=serve+call+stop uses=supervisor",
    );
    assert_eq!(plan.errors, Vec::<String>::new());
    assert!(plan.services[0].unsupervised);
    assert_eq!(plan.services[0].args, "");
    assert!(!plan.services[1].unsupervised);
    assert_eq!(plan.services[1].limits.caps & cap::STOP, cap::STOP);
    assert_eq!(plan.services[1].uses, [SUPERVISOR]);
    for bad in [
        "x caps=serve unsupervised",
        "llm caps=serve
x unsupervised uses=llm",
    ] {
        assert_eq!(parse_plan(bad).errors.len(), 1, "{bad}");
    }
}

#[test]
fn program_names_the_program_and_is_not_passed_on() {
    let plan = parse_plan(kabi::plan::NOTES_TEST);
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    let notes = &plan.services[0];
    assert_eq!(
        (notes.name.as_str(), notes.program.as_str()),
        ("notes", "comphost")
    );
    assert_eq!(notes.limits.caps, cap::SERVE | cap::COMPONENT);
    assert_eq!(notes.limits.max_pages, 8192);
    assert_eq!(notes.args, "version=1 probation_calls=3 probation_ms=0");
    let notes = &parse_plan(&kabi::plan::notes("2", None)).services[0];
    assert_eq!(notes.args, "version=2");
    // Without the key, a service runs the program of its own name.
    let plan = parse_plan(&alloc::format!(
        "llm caps=serve
{}",
        kabi::plan::SHELL
    ));
    let shell = &plan.services[1];
    assert_eq!(shell.program, "shell");
}

#[test]
fn a_service_cannot_run_the_supervisor_or_a_bad_program_name() {
    let plan = parse_plan("evil program=supervisor\nodd program=a/b\nok program=comphost");
    assert_eq!(plan.services.len(), 1);
    assert_eq!(plan.errors.len(), 2, "{:?}", plan.errors);
    assert!(plan.errors[0].contains("supervisor"));
}

#[test]
fn the_edit_service_and_its_test_plan_parse() {
    let line = alloc::format!(
        "{} host=10.0.2.2:5000 token=0123456789abcdef0123456789abcdef",
        kabi::plan::EDIT_BASE
    );
    let plan = parse_plan(&line);
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    let edit = &plan.services[0];
    assert_eq!(
        (edit.name.as_str(), edit.program.as_str()),
        ("edit", "edit")
    );
    assert_eq!(edit.limits.caps, cap::SERVE | cap::PUBLISH);
    assert_eq!(
        edit.args,
        "host=10.0.2.2:5000 token=0123456789abcdef0123456789abcdef"
    );
    let notes = &parse_plan(kabi::plan::NOTES_EDIT_TEST).services[0];
    assert_eq!(notes.args, "version=2 probation_calls=3 probation_ms=0");
    assert_eq!(notes.limits.caps, cap::SERVE | cap::COMPONENT);
}

#[test]
fn uses_gives_routes_to_services_listed_before() {
    let plan = parse_plan("llm caps=serve\nnotes caps=serve\nshell caps=serve+call uses=llm+notes");
    assert_eq!(plan.errors, Vec::<String>::new());
    assert_eq!(plan.services[2].uses, ["llm", "notes"]);
    assert!(plan.services[0].uses.is_empty());
    assert_eq!(plan.services[2].args, "");
}

#[test]
fn routes_form_no_cycle_and_never_reach_the_shell_or_edit() {
    for (bad, why) in [
        (
            "a caps=serve+call uses=b\nb caps=serve+call uses=a",
            "not listed before",
        ),
        ("a caps=serve+call uses=a", "no route may go to a"),
        (
            "shell caps=serve\nx caps=call uses=shell",
            "no route may go to shell",
        ),
        (
            "edit caps=serve\nx caps=call uses=edit",
            "no route may go to edit",
        ),
        ("llm caps=serve\nx caps=call uses=llm+llm", "twice"),
        (
            "llm caps=serve\nx caps=serve uses=llm",
            "needs the call capability",
        ),
    ] {
        let plan = parse_plan(bad);
        assert!(
            plan.errors.iter().any(|e| e.contains(why)),
            "{bad}: {:?}",
            plan.errors
        );
    }
}

#[test]
fn suspend_and_resume_need_a_once_or_a_rule() {
    let once = Proof::Once(Once { badge: 1, id: 2 });
    for request in [Request::Suspend("notes"), Request::Resume("notes")] {
        assert_eq!(proof_fits(request, Proof::Rule), Ok(()));
        assert_eq!(proof_fits(request, once), Ok(()));
        for proof in [Proof::None, Proof::Entry(2), Proof::Undo(2)] {
            assert!(proof_fits(request, proof).is_err(), "{request:?} {proof:?}");
        }
        assert_eq!(
            Request::parse(&request.encode(Proof::Rule)),
            Some((request, Proof::Rule))
        );
    }
    assert_eq!(
        Request::Suspend("notes").call().as_deref(),
        Some("app_suspend(\"notes\")")
    );
    assert_eq!(
        Request::Resume("notes").call().as_deref(),
        Some("app_resume(\"notes\")")
    );
    let records = vec![Record::new("shell"), Record::new("notes")];
    assert_eq!(
        check(Request::Suspend("shell"), &records),
        Err("err protected")
    );
    assert_eq!(check(Request::Suspend("notes"), &records), Ok(1));
}
