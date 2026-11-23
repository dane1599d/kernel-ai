use super::*;
use alloc::vec;

mod requests;
mod system;

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
    // P5 task 1: RESOLVE is a right on a netdest= handle, no longer a cap word.
    let plan = parse_plan("netcheck caps=resolve");
    assert!(plan.services.is_empty());
    assert_eq!(cap::GRANTABLE & cap::RESOLVE, 0);
}

#[test]
fn a_netdest_line_names_its_destination_and_is_bound_to_its_program() {
    let plan = parse_plan("weather netdest=proto:tcp,ports:443,hosts:api.weather.example");
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    let dest = plan.services[0].netdest.as_ref().unwrap();
    assert!(dest.allows_name("api.weather.example"));
    assert!(!dest.allows_name("weather.example"));
    let grants = plan.grants("weather", "weather").unwrap();
    assert_eq!(grants.netdest.as_ref(), Some(dest));
    // Only comphost is told it as an argument; others' arguments keep their room.
    assert_eq!(plan.services[0].args, "");
    let hosted = parse_plan(
        "weather program=comphost netdest=proto:tcp,ports:443,hosts:api.weather.example other=1",
    );
    assert!(hosted.errors.is_empty(), "{:?}", hosted.errors);
    assert_eq!(
        hosted.services[0].args,
        "other=1 netdest=proto:tcp,ports:443,hosts:api.weather.example"
    );
    assert!(plan.grants("weather", "other").is_err());
    assert_eq!(plan.grants("nobody", "nobody").unwrap().netdest, None);
}

#[test]
fn only_a_line_that_says_netdest_llm_gets_the_llm_servers_destination() {
    let plan = parse_plan(
        "llm caps=serve+net netdest=llm
web caps=serve+net
net-free",
    );
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    let llm = plan.grants("llm", "llm").unwrap();
    assert!(llm.llm_dest);
    assert_eq!(llm.netdest, None);
    assert!(
        !plan.grants("web", "web").unwrap().llm_dest,
        "caps=net alone"
    );
    assert!(!plan.grants("net-free", "net-free").unwrap().llm_dest);
    // Bound to its program, as every netdest= is.
    assert!(plan.grants("llm", "other").is_err());
    assert_eq!(plan.services[0].args, "", "not passed on as an argument");
}

#[test]
fn a_bad_or_doubled_netdest_fails_its_line() {
    for line in [
        "a netdest=hosts:com",
        "a netdest=proto:sctp",
        "a netdest=hosts:a.example netdest=hosts:b.example",
        "a netdest=hosts:a.example unsupervised",
        "a netdest=llm netdest=hosts:a.example",
        "a netdest=hosts:a.example netdest=llm",
        "a netdest=llm unsupervised",
        // Only the `llm` line running `llm`, with net in its ceiling, may say
        // netdest=llm, once and exactly so (the manager's review of #779).
        "llm caps=serve+net netdest=llm netdest=llm",
        "llm caps=serve+net netdest=LLM",
        "llm caps=serve+net netdest=llm,ports:443",
        "llm caps=serve netdest=llm",
        "llm netdest=llm",
        "web caps=serve+net netdest=llm",
        "llm program=other caps=serve+net netdest=llm",
    ] {
        let plan = parse_plan(line);
        assert!(plan.services.is_empty(), "{line}");
        assert_eq!(plan.errors.len(), 1, "{line}");
    }
}

#[test]
fn the_editor_comphost_line_names_the_edit_capability() {
    let plan =
        parse_plan("editor program=comphost caps=serve+component+call+edit version=1").services;
    assert_eq!(
        plan[0].limits.caps,
        cap::SERVE | cap::COMPONENT | cap::CALL | cap::EDIT
    );
    assert_eq!(plan[0].args, "version=1");
    assert_ne!(cap::GRANTABLE & cap::EDIT, 0);
}

/// Backlog item 13: the restart from inside the OS is the `power` word's.
#[test]
fn the_power_word_gives_the_power_capability() {
    let plan = parse_plan("shell caps=serve+call+power").services;
    assert_eq!(plan[0].limits.caps, cap::SERVE | cap::CALL | cap::POWER);
    assert_ne!(cap::GRANTABLE & cap::POWER, 0);
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
        "llm caps=serve+net burst_ms=10000 pages=1024 host=10.0.2.2:8080 model=org/model-7b\n{}\n{}",
        kabi::plan::CC,
        kabi::plan::with_updated(kabi::plan::SHELL, "")
    );
    let plan = parse_plan(&text);
    assert_eq!(plan.errors, Vec::<String>::new());
    // cc holds no authority but its service endpoint.
    let cc = &plan.services[1];
    assert_eq!((cc.name.as_str(), cc.limits.caps), ("cc", cap::SERVE));
    assert!(cc.uses.is_empty());
    // It starts when the shell first calls it, not at boot.
    assert_eq!(cc.start, Start::Demand);
    // `fetch`, `send` (fetch in its send role), then `updated`, the model
    // channel, just before the shell: `updated`'s one route is to llm, and it
    // alone holds the model store.
    let fetch = &plan.services[2];
    assert_eq!((fetch.name.as_str(), fetch.start), ("fetch", Start::Demand));
    let send = &plan.services[3];
    assert_eq!(
        (send.name.as_str(), send.program.as_str(), send.start),
        ("send", "fetch", Start::Demand)
    );
    assert_eq!(send.args, "role=post");
    assert_eq!(send.limits.caps, fetch.limits.caps);
    let updated = &plan.services[4];
    assert_eq!(updated.name, "updated");
    assert_eq!(updated.uses, ["llm"]);
    assert!(updated.dirs.contains(&Folder::Models));
    let shell = &plan.services[5];
    assert_eq!(shell.name, "shell");
    assert_eq!(
        shell.limits.caps,
        cap::SERVE | cap::CALL | cap::POLICY | cap::STOP | cap::SCRIPT | cap::POWER
    );
    assert_eq!(
        shell.uses,
        ["llm", SUPERVISOR, "cc", "updated", "fetch", "send"]
    );
    assert_eq!(shell.limits.stack_pages, 512);
    assert!(shell.limits.stack_pages < shell.limits.max_pages);
    assert_eq!(
        plan.services[0].args,
        "host=10.0.2.2:8080 model=org/model-7b"
    );
}

/// The shell finds each route after `updated` by the name `routes=` gives it, so
/// every shell line's `routes=` must name exactly those routes, in `uses=`'s
/// order: a route added anywhere but last, or named wrong, fails here.
#[test]
fn every_shell_lines_routes_are_named_in_order() {
    for line in kabi::plan::SHELL_LINES {
        let text = alloc::format!(
            "llm caps=serve\ndesktop caps=serve\npkg caps=serve\n{}\n{}",
            kabi::plan::CC,
            kabi::plan::with_updated(line, "")
        );
        let plan = parse_plan(&text);
        assert_eq!(plan.errors, Vec::<String>::new(), "{line}");
        let shell = plan.services.iter().find(|s| s.name == "shell").unwrap();
        assert_eq!(
            &shell.uses[..kabi::plan::SHELL_ROUTES.len()],
            ["llm", SUPERVISOR, "cc", "updated"]
        );
        let routes = shell
            .args
            .split_whitespace()
            .find_map(|word| word.strip_prefix("routes="))
            .unwrap_or("");
        let after: Vec<&str> = shell.uses[kabi::plan::SHELL_ROUTES.len()..]
            .iter()
            .map(String::as_str)
            .collect();
        let named: Vec<&str> = routes.split('+').filter(|w| !w.is_empty()).collect();
        assert_eq!(named, after, "{line}");
        for (at, name) in after.iter().enumerate() {
            assert_eq!(
                kabi::plan::named_route(routes, name),
                Some(kabi::plan::SHELL_ROUTES.len() + at)
            );
        }
    }
    assert_eq!(kabi::plan::named_route("desktop+pkg", "pkg"), Some(5));
    assert_eq!(kabi::plan::named_route("desktop", "pkg"), None);
    assert_eq!(kabi::plan::named_route("", ""), None);
    assert_eq!(kabi::plan::named_route("+", ""), None);
    assert_eq!(kabi::plan::named_route("desktop++pkg", "pkg"), Some(5));
    assert_eq!(kabi::plan::named_route("+desktop", "desktop"), Some(4));
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
    // comphost's apps get their saved state's folder the same way.
    let apps = parse_plan("notes program=comphost dir=apps version=2");
    assert!(apps.errors.is_empty(), "{:?}", apps.errors);
    assert_eq!(
        apps.grants("notes", "comphost").unwrap().dirs,
        [Folder::Apps]
    );
    // The model store is `updated`'s alone, as the trust store and updates are.
    let updated = parse_plan("updated caps=update dir=trust+updates+settings+models");
    assert!(updated.errors.is_empty(), "{:?}", updated.errors);
    let dirs = [
        Folder::Trust,
        Folder::Updates,
        Folder::Settings,
        Folder::Models,
    ];
    assert_eq!(updated.grants("updated", "updated").unwrap().dirs, dirs);
    assert_eq!(Folder::Models.badge(), kabi::fs::MODELS_ROOT);
    assert!(!Folder::Models.read_only());
    let other = parse_plan("pkg dir=models");
    assert!(
        !other.errors.is_empty() || other.grants("pkg", "pkg").is_err(),
        "{:?}",
        other.errors
    );
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
            keep: true,
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
            keep: false,
            pid: 0,
            started_ms: 0,
        },
    ];
    assert_eq!(decode(encode(&records).as_bytes()), records);
    assert!(decode(b"").is_empty());
    assert!(decode(b"garbage").is_empty());
    assert!(decode(&[0xFF, 0xFE]).is_empty());
    let damaged = alloc::format!("{STATE_MAGIC}\ncrasher x 1 0 0 0 0 0\nhog 2 0 0 0 0 7 99");
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
    // Version 4 had no keep flag: its records carry over, none kept, a stop kept.
    let old = decode(
        b"supervisor-state 4
hog 2 1 0 1 7 99 5",
    );
    assert_eq!(old.len(), 1);
    assert_eq!(
        (
            old[0].stopped,
            old[0].keep,
            old[0].pid,
            old[0].recent.clone()
        ),
        (true, false, 7, vec![5])
    );
}
