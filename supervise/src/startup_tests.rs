//! What a plan line starts with must fit STARTUP's list, and only the shell
//! may reach `fetch` in its send role (`kabi::plan::SEND`).

use super::*;
use kabi::handle::MAX_STARTUP;

/// Every plan the OS builds with a shell: each shell line with `fetch`,
/// `send` and `updated` before it, and the system plan with and without
/// `kai-data`.
fn plans() -> Vec<String> {
    let mut plans: Vec<String> = kabi::plan::SHELL_LINES
        .iter()
        .map(|line| {
            alloc::format!(
                "llm caps=serve\ndesktop caps=serve\npkg caps=serve\n{}\n{}",
                kabi::plan::CC,
                kabi::plan::with_updated(line, "")
            )
        })
        .collect();
    for files in [false, true] {
        plans.push(kabi::plan::system::plan("llm caps=serve", files));
    }
    plans
}

#[test]
fn every_planned_line_fits_startup() {
    for text in plans() {
        let plan = parse_plan(&text);
        assert_eq!(plan.errors, Vec::<String>::new(), "{text}");
        for service in &plan.services {
            assert!(
                service.startup_handles() <= MAX_STARTUP,
                "{}: {}",
                service.name,
                service.startup_handles()
            );
        }
    }
    // The system's shell fills STARTUP: its five caps, its two folders, eight
    // routes and its ceiling. One handle more is refused, so a new route or
    // folder for it needs one given up first.
    let plan = parse_plan(&kabi::plan::system::plan("llm caps=serve", true));
    let shell = plan.services.iter().find(|s| s.name == "shell").unwrap();
    assert_eq!(shell.startup_handles(), MAX_STARTUP);
    assert_eq!(cap::startup_handles(shell.limits.caps), 5);
}

#[test]
fn a_line_over_startup_is_refused() {
    let services: Vec<String> = (0..MAX_STARTUP).map(|n| alloc::format!("s{n}")).collect();
    let mut text: String = services
        .iter()
        .map(|name| alloc::format!("{name} caps=serve\n"))
        .collect();
    // CALL gives no handle of its own: sixteen routes fit, and with a
    // ceiling as well, seventeen do not.
    let uses = services.join("+");
    text += &alloc::format!("fits caps=call uses={uses}\n");
    text +=
        &alloc::format!("over caps=call uses={uses} netdest=proto:tcp,ports:443,hosts:a.test\n");
    let plan = parse_plan(&text);
    assert!(plan.services.iter().any(|s| s.name == "fits"));
    assert_eq!(
        plan.errors,
        [alloc::format!(
            "over: 17 startup handles, more than the {MAX_STARTUP} a process is given"
        )]
    );
}

#[test]
fn only_the_shell_may_reach_the_send_service() {
    let lines = alloc::format!("{}\n{}\n", kabi::plan::FETCH, kabi::plan::SEND);
    // An app's host, a test program, any line but the shell's: refused.
    for line in [
        "app program=comphost caps=serve+call uses=fetch+send",
        "check program=httpscheck caps=call uses=send",
        "shell2 program=comphost caps=call uses=send",
        // The shell's program under another name, and another program under
        // the shell's name.
        "kai2 program=shell caps=call uses=send",
        "shell program=comphost caps=call uses=send",
    ] {
        let plan = parse_plan(&alloc::format!("{lines}{line}"));
        let name = line.split(' ').next().unwrap();
        assert_eq!(
            plan.errors,
            [alloc::format!(
                "{name}: only the shell may use send, which sends"
            )],
            "{line}"
        );
    }
    // `fetch` itself, and the shell to both.
    let plan = parse_plan(&alloc::format!(
        "{lines}app program=comphost caps=serve+call uses=fetch\nshell caps=call uses=fetch+send"
    ));
    assert_eq!(plan.errors, Vec::<String>::new());
    // Whatever its name, a line running `fetch` with role=post sends.
    let plan = parse_plan(
        "out program=fetch caps=serve+net role=post\napp program=comphost caps=call uses=out",
    );
    assert_eq!(
        plan.errors,
        ["app: only the shell may use out, which sends"]
    );
}
