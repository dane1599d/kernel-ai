//! Resource profiles (P3 task 12, host side): a switch sets each process's class
//! and quota as the profile states, and `undo` puts every one back.

use super::*;
use kaiprofile::Class;

/// A host running `game`, `notes` and `build-1` beside the system, with `game`
/// focused.
fn host() -> Rc<FakeHost> {
    let host = FakeHost::new();
    host.lists_services.set(true);
    *host.procs.borrow_mut() = alloc::vec![
        String::from("game"),
        String::from("notes"),
        String::from("build-1"),
    ];
    *host.focused.borrow_mut() = Some(String::from("game"));
    host
}

fn state(host: &FakeHost, process: &str) -> (Class, u64) {
    host.profile_state
        .borrow()
        .get(process)
        .copied()
        .unwrap_or((Class::Normal, 4096))
}

#[test]
fn a_switch_sets_what_the_profile_states_and_undo_puts_it_back() {
    let host = host();
    let mut shell = shell(&host);
    assert_eq!(
        shell.eval("profile"),
        ok("default (now)\ngaming\nai-dev\nquiet")
    );
    assert_eq!(
        shell.eval("profile gaming"),
        ok("Done: profile gaming (was default). undo")
    );
    assert_eq!(state(&host, "game"), (Class::Normal, 4096));
    assert_eq!(state(&host, "notes"), (Class::Background, 4096));
    assert_eq!(state(&host, "build-1"), (Class::Background, 4096));
    // The model service is never in a profile's settings.
    assert!(!host.profile_state.borrow().contains_key("llm"));
    assert!(shell.eval("profile").text.contains("gaming (now)"));
    assert_eq!(shell.eval("undo"), ok("Undone: profile back to default"));
    assert_eq!(state(&host, "notes"), (Class::Normal, 4096));
    assert!(shell.eval("profile").text.starts_with("default (now)"));
    // `undo` again redoes the switch.
    assert_eq!(shell.eval("undo"), ok("Undone: profile back to gaming"));
    assert_eq!(state(&host, "notes"), (Class::Background, 4096));
    let actlog = shell.eval("log actlog").text;
    assert!(
        actlog.contains(
            "profile_set owner=supervisor res=profile before=default after=gaming inv=profile:"
        ),
        "{actlog}"
    );
}

#[test]
fn an_unknown_profile_or_no_supervisor_changes_nothing() {
    let host = host();
    let mut shell = shell(&host);
    assert_eq!(
        shell.eval("profile turbo"),
        Reply::err("no profile turbo; `profile` lists them")
    );
    assert!(host.profile_state.borrow().is_empty());
    assert_eq!(shell.eval("undo"), Reply::err("nothing to undo"));
}

#[test]
fn a_standing_instruction_switches_on_focus_and_back_on_close() {
    let host = host();
    host.triggers_on.set(true);
    let mut shell = shell(&host);
    let rule = "on focus(game) -> profile_set(gaming) until close(game)";
    shell.eval(&format!("rules_add {rule}"));
    let call = format!("rules_add(\"{rule}\")");
    let once = host.mint(1, &call);
    assert!(shell.eval(&format!("answer 1 allow once={once}")).ok);
    let focus = crate::standing::Event {
        when: crate::standing::When {
            kind: crate::standing::Kind::Focus,
            app: String::from("game"),
        },
        cause: None,
    };
    assert_eq!(shell.on_event(&focus), None);
    assert_eq!(state(&host, "notes"), (Class::Background, 4096));
    assert!(shell.eval("profile").text.contains("gaming (now)"));
    let close = crate::standing::Event {
        when: crate::standing::When {
            kind: crate::standing::Kind::Close,
            app: String::from("game"),
        },
        cause: None,
    };
    assert_eq!(shell.on_event(&close), None);
    assert_eq!(state(&host, "notes"), (Class::Normal, 4096));
    assert!(shell.eval("profile").text.starts_with("default (now)"));
    // The owner never ran it: the shell switches profiles itself.
    assert!(host.triggers.runs.borrow().is_empty());
    let actlog = shell.eval("log actlog").text;
    assert!(
        actlog.contains("form=standing tier=run consent=standing:1"),
        "{actlog}"
    );
}
