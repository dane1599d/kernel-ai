//! Package runs through `pkg`, and the v0.1 system plan's lines.

use super::*;

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

#[test]
fn may_run_asks_what_a_run_would_get_and_starts_nothing() {
    let plan = parse_plan(&format!(
        "{RUN_PLAN}\nnotes-window program=comphost caps=serve+component start=demand"
    ));
    let pids = [Some(10), Some(11), None, Some(13), None];
    let held: Vec<(&ServiceSpec, Option<u64>)> = plan.services.iter().zip(pids).collect();
    assert_eq!(parse_may_run("may-run notes"), Some("notes"));
    assert_eq!(encode_may_run("notes"), "may-run notes");
    assert_eq!(check_run_name("notes", &held, 11), Ok(2));
    // A window the plan runs from the image's version: pkg adds no version of it.
    assert_eq!(check_run_name("notes-window", &held, 11), Err(NOT_FROM_PKG));
    assert_eq!(check_run_name("files-window", &held, 11), Err(NOT_PLANNED));
    // Only the store's holder asks.
    assert!(check_run_name("notes", &held, 13).is_err());
    for bad in ["may-run", "may-run notes x", "may-run no/tes", "may notes"] {
        assert_eq!(parse_may_run(bad), None, "{bad}");
    }
}

#[test]
fn a_withdraw_reads_as_a_run_and_is_gated_as_one() {
    let plan = parse_plan(RUN_PLAN);
    let pids = [Some(10), Some(11), None, Some(13)];
    let held: Vec<(&ServiceSpec, Option<u64>)> = plan.services.iter().zip(pids).collect();
    let withdraw = Run::parse_withdraw("withdraw notes version=p-0a1b2c3d4e5f").unwrap();
    assert_eq!(
        withdraw.encode_withdraw(),
        "withdraw notes version=p-0a1b2c3d4e5f"
    );
    assert_eq!(check_run(withdraw, &held, 11), Ok(2));
    for caller in [10, 13, 0, 99] {
        assert!(check_run(withdraw, &held, caller).is_err(), "{caller}");
    }
    // Each verb reads only itself.
    assert_eq!(Run::parse("withdraw notes version=p-0a1b2c3d4e5f"), None);
    assert_eq!(
        Run::parse_withdraw("run notes version=p-0a1b2c3d4e5f"),
        None
    );
    assert_eq!(Run::parse_withdraw("withdraw notes"), None);
}

/// The v0.1 system (`kabi::plan::system`), the default boot's and
/// `desk-default`'s: with and without `kai-data`, every line parses, every
/// route names a service planned before it, and every app starts on demand.
#[test]
fn the_v0_1_system_plan_parses_and_its_routes_come_first() {
    let llm = "llm caps=serve+net burst_ms=10000 pages=1024 host=10.0.2.2:8080 model=org/model-7b";
    for files in [true, false] {
        let plan = parse_plan(&kabi::plan::system::plan(llm, files));
        assert_eq!(plan.errors, Vec::<String>::new(), "files={files}");
        for (at, service) in plan.services.iter().enumerate() {
            for target in &service.uses {
                let earlier = plan.services[..at].iter().any(|s| s.name == *target);
                assert!(
                    earlier || target == "supervisor",
                    "{}'s route to {target} (files={files})",
                    service.name
                );
            }
        }
        let named = |name: &str| plan.services.iter().find(|s| s.name == name);
        for app in kabi::plan::system::APPS {
            let name = app.split(' ').next().unwrap_or_default();
            let service = named(name).expect("every app is planned");
            assert_eq!(service.start, Start::Demand, "{name}");
            assert_eq!(service.program, "comphost", "{name}");
        }
        // The shell's routes after `updated`: `desktop`, and with `pkg` (where the
        // editor's builds install) `pkg` after it.
        let shell = named("shell").expect("the shell is planned");
        let tail: &[&str] = if files {
            &["desktop", "pkg", "fetch", "send"]
        } else {
            &["desktop", "fetch", "send"]
        };
        assert_eq!(shell.uses[shell.uses.len() - tail.len()..], *tail);
        assert_eq!(named("pkg").is_some(), files);
        assert_eq!(named("fs").is_some(), files);
        let desktop = named("desktop").expect("the desktop is planned");
        assert_eq!(desktop.uses.contains(&String::from("pkg")), files);
        // `updated` reaches Hugging Face's resolve host and its file storage,
        // and the repository's Pages host for the chunk lists, by name on 443
        // for the model download (P5 task 9), and nothing else.
        let updated = named("updated").expect("updated is planned");
        let dest = updated.netdest.as_ref().expect("updated has a NetDest");
        assert_eq!(
            dest.to_string(),
            "proto:tcp,addrs:none,ports:443,hosts:huggingface.co+cdn.hf.co+dane1599d.github.io"
        );
        // The model manager's window, with `kai-data` only, on demand.
        let models = named("models-window");
        assert_eq!(models.is_some(), files);
        if let Some(models) = models {
            assert_eq!(models.start, Start::Demand);
            assert_eq!(models.uses, ["desktop", "updated"]);
        }
    }
    // The System monitor's line is main's own, started on demand.
    assert!(
        kabi::plan::system::APPS
            .contains(&alloc::format!("{} start=demand", kabi::plan::MONITOR_WINDOW).as_str())
    );
}

#[test]
fn the_editor_holds_the_edit_capability_by_name_and_routes_to_desktop() {
    let plan = parse_plan(&alloc::format!(
        "desktop caps=display+input+serve\n{}",
        kabi::plan::EDITOR
    ));
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    let editor = &plan.services[1];
    // The kernel pins `cap::EDIT` to the service of this name running comphost.
    assert_eq!(editor.name, "editor");
    assert_eq!(editor.program, "comphost");
    assert_eq!(editor.uses, ["desktop"]);
    assert_eq!(
        editor.limits.caps,
        cap::SERVE | cap::COMPONENT | cap::CALL | cap::EDIT
    );
    // Without `desktop` before it the line is an error, not a route elsewhere.
    assert!(!parse_plan(kabi::plan::EDITOR).errors.is_empty());
}

#[test]
fn the_monitor_s_routes_are_desktop_then_llm_and_the_name_is_the_grant() {
    let plan = parse_plan(&alloc::format!(
        "desktop caps=display+input+serve\nllm caps=serve+net\n{}",
        kabi::plan::MONITOR_WINDOW
    ));
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    let monitor = &plan.services[2];
    assert_eq!(monitor.name, "monitor-window");
    assert_eq!(monitor.program, "comphost");
    // comphost reads the first route as desktop's and the second as llm's.
    assert_eq!(monitor.uses, ["desktop", "llm"]);
    // The supervisor takes the other order too; only this test says it is wrong
    // for comphost, which would send `health` to desktop.
    let swapped = kabi::plan::MONITOR_WINDOW.replace("desktop+llm", "llm+desktop");
    let plan = parse_plan(&alloc::format!(
        "desktop caps=display+input+serve\nllm caps=serve+net\n{swapped}"
    ));
    assert_eq!(plan.services[2].uses, ["llm", "desktop"]);
    // Without the services before it the line is an error, not a route elsewhere.
    assert!(!parse_plan(kabi::plan::MONITOR_WINDOW).errors.is_empty());
}

#[test]
fn the_models_window_s_routes_are_desktop_then_updated_and_the_name_is_the_grant() {
    let plan = parse_plan(&alloc::format!(
        "desktop caps=display+input+serve\nupdated caps=update+serve+call\n{}",
        kabi::plan::MODELS_WINDOW
    ));
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    let models = &plan.services[2];
    // comphost grants `kai:models` and `updated` answers `models` by this name.
    assert_eq!(models.name, "models-window");
    assert_eq!(models.program, "comphost");
    // comphost reads the first route as desktop's and the second as updated's.
    assert_eq!(models.uses, ["desktop", "updated"]);
    assert_eq!(models.limits.caps, cap::SERVE | cap::COMPONENT | cap::CALL);
    // Without the services before it the line is an error, not a route elsewhere.
    assert!(!parse_plan(kabi::plan::MODELS_WINDOW).errors.is_empty());
}

#[test]
fn hosts_any_sits_on_the_shell_line_alone_and_only_for_https() {
    let plan = parse_plan("shell caps=serve netdest=proto:tcp,ports:443,hosts:any");
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    let dest = plan.services[0].netdest.as_ref().unwrap();
    assert!(dest.any_host && dest.allows_name("en.wikipedia.org"));
    for line in [
        // Any other line: an app, a package, a service.
        "weather netdest=proto:tcp,ports:443,hosts:any",
        "fetch netdest=proto:tcp,ports:443,hosts:any",
        "pkg start=pkg netdest=proto:tcp,ports:443,hosts:any",
        // The shell's name on another program.
        "shell program=notes netdest=proto:tcp,ports:443,hosts:any",
        // Not HTTPS alone.
        "shell netdest=proto:tcp,ports:any,hosts:any",
        "shell netdest=proto:tcp,ports:80+443,hosts:any",
        "shell netdest=proto:udp,ports:443,hosts:any",
        "shell netdest=proto:tcp,addrs:any,ports:443,hosts:any",
    ] {
        let plan = parse_plan(line);
        assert!(plan.services.is_empty(), "{line}");
        assert!(
            plan.errors[0].contains("hosts:any"),
            "{line}: {:?}",
            plan.errors
        );
    }
}
