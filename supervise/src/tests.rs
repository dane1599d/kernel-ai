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
fn a_lines_folders_go_only_to_its_own_program() {
    let plan = parse_plan(
        "fs caps=serve\npkg dir=pkg\nstore program=pkgd dir=pkg\nnotes caps=call uses=fs",
    );
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    assert_eq!(plan.grants("pkg", "pkg").unwrap().dirs, [Folder::Pkg]);
    // The name kept, the program swapped: refused, never started without it.
    let refused = plan.grants("pkg", "notes").unwrap_err();
    assert!(refused.contains("for program pkg, not notes"), "{refused}");
    // A line's `program=` is the program its folder goes to, not its name.
    assert_eq!(plan.grants("store", "pkgd").unwrap().dirs, [Folder::Pkg]);
    assert!(plan.grants("store", "store").is_err());
    // A line with no folder keeps today's rule; routes come as before.
    let notes = Grants {
        uses: vec!["fs".into()],
        ..Grants::default()
    };
    assert_eq!(plan.grants("notes", "comphost"), Ok(notes));
    assert_eq!(plan.grants("absent", "pkg"), Ok(Grants::default()));
}

#[test]
fn boot_files_go_only_to_the_lines_own_program() {
    let plan = parse_plan("pkg dir=pkg bootfiles=pkgrepo\nimager program=pkg bootfiles=pkgrepo");
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    // `boot=` stays the program's own word, as fs-powercut's filecheck uses it.
    let churn = parse_plan("filecheck caps=files step=churn boot=1");
    assert!(churn.errors.is_empty(), "{:?}", churn.errors);
    assert_eq!(churn.services[0].args, "step=churn boot=1");
    assert_eq!(churn.services[0].boot, None);
    assert_eq!(plan.services[0].boot, Some(BootFiles::Pkgrepo));
    assert_eq!(
        plan.grants("pkg", "pkg").unwrap().boot,
        Some(BootFiles::Pkgrepo)
    );
    let refused = plan.grants("imager", "imager").unwrap_err();
    assert!(
        refused.contains("its bootfiles= is for program pkg"),
        "{refused}"
    );
    assert!(plan.grants("pkg", "comphost").is_err());
    assert_eq!(parse_plan("pkg").services[0].boot, None);
    for bad in [
        "pkg bootfiles=",
        "pkg bootfiles=pkgrepo/",
        "pkg bootfiles=Pkgrepo",
        "pkg bootfiles=comp",
        "pkg bootfiles=pkgrepo+pkgrepo",
        "pkg bootfiles=pkgrepo bootfiles=pkgrepo",
        "pkg bootfiles=pkgrepo unsupervised",
    ] {
        let plan = parse_plan(bad);
        assert!(plan.services.is_empty() && plan.errors.len() == 1, "{bad}");
    }
}

#[test]
fn boot_files_reach_only_beneath_their_folder() {
    let repo = BootFiles::Pkgrepo;
    for good in [
        "pkgrepo/metadata/1.root.json",
        "pkgrepo/targets/0a1b.notes-1.0.0.kpkg",
        "pkgrepo/x",
    ] {
        assert!(repo.allows(good), "{good}");
    }
    let long = format!("pkgrepo/{}", "a".repeat(MAX_BOOT_PATH));
    for bad in [
        "",
        "pkgrepo",
        "pkgrepo/",
        "/pkgrepo/x",
        "pkgrepo//x",
        "pkgrepo/x/",
        "pkgrepo/../kernel.lines",
        "pkgrepo/metadata/../../kernel.lines",
        "pkgrepo/./x",
        "pkgrepo/..",
        "pkgrepo\\x",
        "pkgrepo/x y",
        "pkgrepo/é",
        "pkgrepox/y",
        "comp/notes/2.wasm",
        "kernel.lines",
        &long,
    ] {
        assert!(!repo.allows(bad), "{bad:?}");
    }
}

#[test]
fn dir_grants_only_the_folders_it_names() {
    let plan = parse_plan("pkg dir=pkg step=install").services;
    assert_eq!(plan[0].dirs, [Folder::Pkg]);
    assert_eq!(plan[0].dirs[0].badge(), kabi::fs::SYSTEM_PKG);
    // A grant is the plan's key, never a program argument.
    assert_eq!(plan[0].args, "step=install");
    assert_eq!(plan[0].limits.caps, 0);
    assert!(parse_plan("pkg").services[0].dirs.is_empty());
    for bad in [
        "pkg dir=",
        "pkg dir=home",
        "pkg dir=Pkg",
        "pkg dir=pkg+pkg",
        "pkg dir=pkg dir=pkg",
        "pkg dir=pkg+",
        "pkg dir=../pkg",
        "pkg dir=system/pkg",
        "pkg dir=pkg unsupervised",
    ] {
        let plan = parse_plan(bad);
        assert!(plan.services.is_empty() && plan.errors.len() == 1, "{bad}");
    }
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

const RUN_PLAN: &str = "fs caps=serve\n\
    pkg dir=pkg bootfiles=pkgrepo caps=call uses=supervisor\n\
    notes program=comphost caps=serve+component start=pkg\n\
    shell caps=call";

#[test]
fn a_start_pkg_line_waits_for_pkg_and_names_no_version() {
    let plan = parse_plan(RUN_PLAN);
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    let notes = plan.services.iter().find(|s| s.name == "notes").unwrap();
    assert_eq!(notes.start, Start::Pkg);
    assert_eq!(notes.args, "");
    assert_eq!(parse_plan("notes").services[0].start, Start::Boot);
    for bad in [
        "notes start=",
        "notes start=boot",
        "notes start=PKG",
        "notes start=pkg start=pkg",
        "notes start=pkg version=2",
        "notes start=pkg unsupervised",
    ] {
        let plan = parse_plan(bad);
        assert!(plan.services.is_empty() && plan.errors.len() == 1, "{bad}");
    }
}

#[test]
fn only_the_store_holder_may_run_a_package_version() {
    let plan = parse_plan(RUN_PLAN);
    let pids = [Some(10), Some(11), None, Some(13)];
    let held: Vec<(&ServiceSpec, Option<u64>)> = plan.services.iter().zip(pids).collect();
    let run = Run::parse("run notes version=p-0a1b2c3d4e5f").unwrap();
    assert_eq!(run.encode(), "run notes version=p-0a1b2c3d4e5f");
    assert_eq!(check_run(run, &held, 11), Ok(2));
    // fs, the shell, the kernel and an unknown pid are not the store's holder.
    for caller in [10, 13, 0, 99] {
        assert!(check_run(run, &held, caller).is_err(), "{caller}");
    }
    // A holder that is not running holds nothing.
    let stopped: Vec<(&ServiceSpec, Option<u64>)> = plan
        .services
        .iter()
        .zip([Some(10), None, None, Some(13)])
        .collect();
    assert!(check_run(run, &stopped, 11).is_err());
    let shell = Run::parse("run shell version=p-0a1b2c3d4e5f").unwrap();
    assert_eq!(
        check_run(shell, &held, 11),
        Err("err not a start=pkg service")
    );
    for bad in [
        "run notes",
        "run notes version=2",
        "run notes version=c-0a1b2c3d4e5f",
        "run notes version=p-0A1B2C3D4E5F",
        "run notes version=p-0a1b2c3d4e5",
        "run notes version=p-0a1b2c3d4e5f extra",
        "run no/tes version=p-0a1b2c3d4e5f",
        "start notes",
    ] {
        assert_eq!(Run::parse(bad), None, "{bad}");
    }
}
