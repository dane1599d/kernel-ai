//! Requests to the supervisor: keep, stop, start, suspend and services; program
//! names, routes and demand starts.

use super::*;

#[test]
fn keep_requests_parse() {
    assert_eq!(keep::parse("keep notes on"), Some(("notes", true)));
    assert_eq!(keep::parse("keep notes off"), Some(("notes", false)));
    assert_eq!(
        keep::parse(&keep::encode("files-window", true)),
        Some(("files-window", true))
    );
    for bad in [
        "keep",
        "keep notes",
        "keep notes yes",
        "keep notes on now",
        "keep no/tes on",
        "keep  on",
        "stop notes",
        "",
    ] {
        assert_eq!(keep::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn only_the_shell_keeps_only_apps_running() {
    let plan = parse_plan(
        "notes program=comphost caps=serve+component\nshell caps=serve\nfiles caps=serve\nllm program=comphost caps=serve",
    );
    assert_eq!(plan.errors, Vec::<String>::new());
    let specs: Vec<&ServiceSpec> = plan.services.iter().collect();
    let records: Vec<Record> = specs.iter().map(|s| Record::new(&s.name)).collect();
    let shell = Some(3);
    assert_eq!(keep::check("notes", &specs, &records, 3, shell), Ok(0));
    // Anyone else, or with no shell running, is refused.
    assert!(keep::check("notes", &specs, &records, 4, shell).is_err());
    assert!(keep::check("notes", &specs, &records, 0, shell).is_err());
    assert!(keep::check("notes", &specs, &records, 3, None).is_err());
    // Only an app's line naps, so only it may be kept.
    assert_eq!(
        keep::check("files", &specs, &records, 3, shell),
        Err("err not an app")
    );
    assert_eq!(
        keep::check("nobody", &specs, &records, 3, shell),
        Err("err unknown service")
    );
    // A protected name is refused even on a comphost line.
    assert_eq!(
        keep::check("llm", &specs, &records, 3, shell),
        Err("err protected")
    );
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
{}
{}",
        kabi::plan::CC,
        kabi::plan::with_updated(kabi::plan::SHELL, "")
    ));
    assert_eq!(plan.errors, Vec::<String>::new());
    assert_eq!(plan.services[1].program, "cc");
    assert_eq!(plan.services[2].program, "fetch");
    assert_eq!(plan.services[3].program, "fetch");
    assert_eq!(plan.services[4].program, "updated");
    let shell = &plan.services[5];
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
fn a_demand_line_waits_for_the_kernels_demand() {
    let plan = parse_plan(
        "notes program=comphost caps=serve start=demand\nshell caps=serve\nodd start=later\ntwice start=demand start=demand\nlone unsupervised start=demand",
    );
    assert_eq!(plan.services.len(), 2, "{:?}", plan.errors);
    assert_eq!(plan.errors.len(), 3, "{:?}", plan.errors);
    assert_eq!(plan.services[0].start, Start::Demand);
    assert_eq!(plan.services[1].start, Start::Boot);
    assert_eq!(parse_demand("demand notes"), Some("notes"));
    for bad in [
        "demand",
        "demand notes x",
        "demand no/tes",
        "start notes",
        "",
    ] {
        assert_eq!(parse_demand(bad), None, "{bad:?}");
    }
    let specs: Vec<&ServiceSpec> = plan.services.iter().collect();
    let mut records = vec![Record::new("notes"), Record::new("shell")];
    let idle = [false, false];
    assert_eq!(
        check_demand("notes", &specs, &records, 0, &idle),
        Ok(Some(0))
    );
    // Only the kernel asks, and only for a start=demand line.
    assert!(check_demand("notes", &specs, &records, 7, &idle).is_err());
    assert!(check_demand("shell", &specs, &records, 0, &idle).is_err());
    assert!(check_demand("nobody", &specs, &records, 0, &idle).is_err());
    // Already starting or running: nothing to do.
    assert_eq!(
        check_demand("notes", &specs, &records, 0, &[true, false]),
        Ok(None)
    );
    records[0].pid = 9;
    assert_eq!(check_demand("notes", &specs, &records, 0, &idle), Ok(None));
    // Stopped by the user: a demand does not undo that.
    records[0].pid = 0;
    records[0].stopped = true;
    assert_eq!(
        check_demand("notes", &specs, &records, 0, &idle),
        Err("err stopped")
    );
}

#[test]
fn a_demand_does_not_restart_a_service_past_its_cap() {
    let plan = parse_plan("notes program=comphost caps=serve start=demand");
    let specs: Vec<&ServiceSpec> = plan.services.iter().collect();
    let mut records = vec![Record::new("notes")];
    records[0].gave_up = true;
    assert_eq!(
        check_demand("notes", &specs, &records, 0, &[false]),
        Err("err gave up")
    );
    // While it backs off it is starting, so a demand adds nothing either.
    records[0].gave_up = false;
    assert_eq!(
        check_demand("notes", &specs, &records, 0, &[true]),
        Ok(None)
    );
}

#[test]
fn start_starts_a_demand_service_not_started_yet() {
    let starts = [Start::Demand, Start::Boot];
    let mut records = vec![Record::new("notes"), Record::new("files")];
    // Never started: `start` starts it, as a demand would.
    assert_eq!(check_for(Request::Start("notes"), &records, &starts), Ok(0));
    // A boot service that runs is not stopped, so `start` is refused as before.
    records[1].pid = 4;
    assert_eq!(
        check_for(Request::Start("files"), &records, &starts),
        Err("err not stopped")
    );
    // Once the demand service runs, `start` is refused like any other.
    records[0].pid = 9;
    assert_eq!(
        check_for(Request::Start("notes"), &records, &starts),
        Err("err not stopped")
    );
    // `check` alone knows no starts: unchanged.
    records[0].pid = 0;
    assert_eq!(
        check(Request::Start("notes"), &records),
        Err("err not stopped")
    );
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

#[test]
fn an_undone_on_demand_start_ends_back_on_demand_with_no_restart_counted() {
    // start (the service waited on demand), its undo, then the exit the undo
    // caused: none of that restarts it or counts against its cap, so a later
    // demand still starts it.
    let spec = parse_plan("send start=demand caps=serve").services;
    let record = Record {
        name: String::from("send"),
        ..Record::default()
    };
    assert_eq!(ending(false, true, false, false), Ending::BackOnDemand);
    // The undo's flag wins over a restart asked for or a stop, as `withdrawn`
    // wins over it.
    assert_eq!(ending(false, true, true, true), Ending::BackOnDemand);
    assert_eq!(ending(true, true, false, false), Ending::Withdrawn);
    assert_eq!(ending(false, false, false, false), Ending::Policy);
    assert_eq!(record.restarts, 0);
    assert!(!record.gave_up && !record.stopped);
    let specs: Vec<&ServiceSpec> = spec.iter().collect();
    assert_eq!(
        check_demand("send", &specs, core::slice::from_ref(&record), 0, &[false]),
        Ok(Some(0))
    );
}
