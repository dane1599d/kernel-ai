//! ADR 0002 D2: every run-tier row needs a test that its undo restores. This
//! one walks every row table agentabi exports that the shell runs (`ROWS`,
//! the desk rows): each run-tier change row whose inverse restores
//! (Restore, Compensate, Snapshot, Trash) has a case here that runs it through
//! its own door on the fake host, sees it logged with its undo, runs `undo`,
//! and sees it put back. A new row without a case fails: "add <row> to CASES".

use super::*;
use crate::actlog::Change;
use agentabi::{Action, Inverse, Tier};
use alloc::vec;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

/// A shell with `calls` queued for its model, on a fresh fake host.
fn with(calls: &[&str]) -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    host.calls
        .borrow_mut()
        .extend(calls.iter().map(|json| call(json)));
    let shell = shell(&host);
    (host, shell)
}

/// Runs the row's case: the host, the shell, and the reply to its line.
type Run = fn() -> (Rc<FakeHost>, Shell, Reply);

/// One line through the door, after `calls` are queued.
fn line(calls: &[&str], before: &[&str], line: &str) -> (Rc<FakeHost>, Shell, Reply) {
    let (host, mut shell) = with(calls);
    for earlier in before {
        assert!(shell.eval(earlier).ok, "{earlier}");
    }
    let reply = shell.eval(line);
    (host, shell, reply)
}

/// One line on a host with a desktop (`ui::desktop`).
fn on_desktop(json: &str, said: &str) -> (Rc<FakeHost>, Shell, Reply) {
    let (host, mut shell) = with(&[json]);
    super::ui::desktop(&host);
    let reply = shell.eval(said);
    (host, shell, reply)
}

/// A standing instruction `rule`, made on its card's Allow, fired by `event`.
fn fired(rule: &str, event: crate::standing::Kind, app: &str) -> (Rc<FakeHost>, Shell, Reply) {
    let host = FakeHost::new();
    host.triggers_on.set(true);
    host.lists_services.set(true);
    *host.procs.borrow_mut() = vec![String::from("game"), String::from("notes")];
    *host.focused.borrow_mut() = Some(String::from("game"));
    let mut shell = shell(&host);
    shell.eval(&format!("rules_add {rule}"));
    let once = host.mint(1, &format!("rules_add(\"{rule}\")"));
    assert!(
        shell.eval(&format!("answer 1 allow once={once}")).ok,
        "{rule}"
    );
    let before = shell.log.last_id();
    let when = crate::standing::When {
        kind: event,
        app: String::from(app),
    };
    shell.on_event(&crate::standing::Event { when, cause: None });
    let reply = match shell.log.last_id() > before {
        true => Reply::ok("fired. undo"),
        false => Reply::err(format!("{rule} did not fire")),
    };
    (host, shell, reply)
}

/// The `goals` agent's badge (tests/goals.rs).
const GOALS: u64 = 2;

/// Goal 1 set on the person's allow and taken by `goals`, then `toggles`
/// (each a `goal_pause` or `goal_resume` call) on the person's line `said`;
/// the reply is the last one's.
fn goal_toggled(toggles: &[&str]) -> (Rc<FakeHost>, Shell, Reply) {
    let set = r#"{"name":"goal_set","arguments":{"text":"tidy the files in notes/q3","steps":10,"tokens":5000,"wall":"2h"}}"#;
    let (host, mut shell) = with(&[set]);
    shell.eval("keep tidying the files in notes/q3 for the next two hours");
    let once = host.mint(1, "goal_set(1)");
    let set = shell.eval(&format!("answer 1 allow once={once}"));
    assert!(set.text.starts_with("Goal 1 set"), "{set:?}");
    assert!(goals_step(&mut shell, "next").starts_with("ok #1 goal-add 1"));
    assert_eq!(goals_step(&mut shell, "report 1 ok"), "ok reported #1");
    let mut reply = Reply::err("no toggle");
    for json in toggles {
        host.calls.borrow_mut().push_back(call(json));
        reply = shell.eval("please pause it or resume it now");
    }
    (host, shell, reply)
}

fn goals_step(shell: &mut Shell, text: &str) -> String {
    super::agentstep::step(shell, GOALS, text)
}

const GOAL_PAUSE: &str = r#"{"name":"goal_pause","arguments":{"id":1}}"#;
const GOAL_RESUME: &str = r#"{"name":"goal_resume","arguments":{"id":1}}"#;

const FS_CREATE: &str = r#"{"name":"fs_create","arguments":{"path":"notes.txt","text":"hello"}}"#;
const MAKE_NOTES: &str = "create notes.txt saying hello";

/// Each row's case.
const CASES: &[(&str, Run)] = &[
    ("set_policy", || {
        line(
            &[r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#],
            &[],
            "switch the scheduler to round robin",
        )
    }),
    ("service_start", || {
        line(
            &[r#"{"name":"service_start","arguments":{"name":"notes"}}"#],
            &[],
            "start notes",
        )
    }),
    ("pkg_install", || line(&[], &[], "install notes")),
    ("pkg_remove", || {
        line(&[], &["install notes"], "remove notes")
    }),
    ("fs_create", || line(&[FS_CREATE], &[], MAKE_NOTES)),
    ("fs_write", || {
        line(
            &[
                FS_CREATE,
                r#"{"name":"fs_write","arguments":{"path":"notes.txt","text":"bye"}}"#,
            ],
            &[MAKE_NOTES],
            "write bye to notes.txt",
        )
    }),
    ("fs_move", || {
        line(
            &[
                FS_CREATE,
                r#"{"name":"fs_move","arguments":{"from":"notes.txt","to":"plan.txt"}}"#,
            ],
            &[MAKE_NOTES],
            "move notes.txt to plan.txt",
        )
    }),
    ("fs_trash", || {
        line(
            &[
                FS_CREATE,
                r#"{"name":"fs_trash","arguments":{"path":"notes.txt"}}"#,
            ],
            &[MAKE_NOTES],
            "trash notes.txt",
        )
    }),
    ("fs_restore", || {
        line(
            &[
                FS_CREATE,
                r#"{"name":"fs_trash","arguments":{"path":"notes.txt"}}"#,
                r#"{"name":"fs_restore","arguments":{"path":"notes.txt"}}"#,
            ],
            &[MAKE_NOTES, "trash notes.txt"],
            "restore notes.txt from the trash",
        )
    }),
    ("ui_open", || {
        on_desktop(
            r#"{"name":"ui_open","arguments":{"app":"notes"}}"#,
            "open notes",
        )
    }),
    ("ui_focus", || {
        on_desktop(
            r#"{"name":"ui_focus","arguments":{"window":"notes"}}"#,
            "switch to notes",
        )
    }),
    ("appearance_set", || {
        line(
            &[r#"{"name":"appearance_set","arguments":{"setting":"theme","value":"light"}}"#],
            &[],
            "switch to the light theme",
        )
    }),
    ("model_use", || {
        let host = FakeHost::new();
        let mut shell = shell(&host);
        let reply = shell.eval("models use qwen3-0.6b");
        (host, shell, reply)
    }),
    ("app_suspend", || {
        fired(
            "on minimize(notes) -> app_suspend(notes)",
            crate::standing::Kind::Minimize,
            "notes",
        )
    }),
    ("app_resume", || {
        fired(
            "on focus(notes) -> app_resume(notes)",
            crate::standing::Kind::Focus,
            "notes",
        )
    }),
    ("goal_pause", || goal_toggled(&[GOAL_PAUSE])),
    ("goal_resume", || goal_toggled(&[GOAL_PAUSE, GOAL_RESUME])),
    ("profile_set", || {
        fired(
            "on focus(game) -> profile_set(gaming)",
            crate::standing::Kind::Focus,
            "game",
        )
    }),
];

/// Rows with no undo case yet, each with why; a row leaves this list when it
/// gets a runner.
const NO_CASE: &[(&str, &str)] = &[
    // An undo itself: tune's own tests undo it (tests/tune.rs).
    ("tune_undo", "it is the undo of tune"),
    // No runner anywhere yet: a call is refused as typed-only.
    ("component_revert", "nothing runs it yet"),
    // Its undo is "nothing installed until the card's yes": a build installs
    // nothing, so there is nothing to put back (tests/appbuild.rs).
    ("app_build", "a build installs nothing"),
    // Settings' click, with its own undo tests (tests/effects.rs).
    ("grant_revoke", "Settings' click, tested in effects.rs"),
    // Two arguments: no standing instruction or line runs them yet.
    ("app_priority", "nothing runs it yet"),
    ("app_mem_limit", "nothing runs it yet"),
];

/// Every run-tier change row that says how it is undone.
fn undoable() -> Vec<&'static Action> {
    agentabi::ROWS
        .iter()
        .chain(&agentabi::DESK_ROWS)
        .filter(|row| agentabi::derive(row) == Tier::Run && !row.facts.read_only)
        .filter(|row| {
            matches!(
                row.inverse,
                Inverse::Restore | Inverse::Compensate(_) | Inverse::Snapshot | Inverse::Trash
            )
        })
        .collect()
}

#[test]
fn every_run_tier_change_row_is_undone_by_undo() {
    for row in undoable() {
        if NO_CASE.iter().any(|(name, _)| *name == row.name) {
            continue;
        }
        let (_, run) = CASES
            .iter()
            .find(|(name, _)| *name == row.name)
            .unwrap_or_else(|| panic!("add {} to CASES", row.name));
        let (_host, mut shell, reply) = run();
        assert!(
            reply.ok && reply.text.ends_with("undo"),
            "{}: {reply:?}",
            row.name
        );
        let made = shell.log.last_id();
        let changes: Vec<Change> = shell.log.get(made).unwrap().changes.clone();
        assert!(
            changes.iter().any(|c| c.inverse.is_some()),
            "{}: logged with no undo: {changes:?}",
            row.name
        );
        let undone = shell.eval("undo");
        // An undo that may lose later work asks first (ui_open's reclose).
        let undone = match undone.text.ends_with("y/n") {
            true => shell.eval("y"),
            false => undone,
        };
        assert!(
            undone.ok && undone.text.starts_with("Undone"),
            "{}: {undone:?}",
            row.name
        );
        // The undo is an entry of its own, which puts each change back.
        let back = shell.log.last_id();
        assert!(back > made, "{}: undo logged nothing", row.name);
        for change in &changes {
            let put = shell
                .log
                .get(back)
                .unwrap()
                .changes
                .iter()
                .find(|c| c.res == change.res)
                .unwrap_or_else(|| panic!("{}: {} not put back", row.name, change.res))
                .clone();
            assert_eq!(put.after, change.before, "{}: {}", row.name, change.res);
        }
    }
}

#[test]
fn no_row_is_both_cased_and_excused_and_every_name_is_a_row() {
    let rows = undoable();
    for (name, _) in CASES {
        assert!(
            rows.iter().any(|r| r.name == *name),
            "{name} is no undoable row"
        );
        assert!(!NO_CASE.iter().any(|(n, _)| n == name), "{name} is both");
    }
    for (name, _) in NO_CASE {
        assert!(
            rows.iter().any(|r| r.name == *name),
            "{name} is no undoable row"
        );
    }
}

/// The log's before and after are words; this checks the goal itself: after
/// `undo` the goal is in the state it was in before the row ran, and `goals`
/// is told so on its queue, the step after the row's own command.
#[test]
fn undo_puts_a_paused_or_resumed_goal_back_and_tells_goals() {
    for (toggles, row_command, before, back_command) in [
        (
            &[GOAL_PAUSE][..],
            "goal-pause 1",
            "running",
            "goal-resume 1",
        ),
        (
            &[GOAL_PAUSE, GOAL_RESUME][..],
            "goal-resume 1",
            "paused",
            "goal-pause 1",
        ),
    ] {
        let (_host, mut shell, reply) = goal_toggled(toggles);
        assert!(reply.ok, "{row_command}: {reply:?}");
        let undone = shell.eval("undo");
        assert!(undone.ok && undone.text.starts_with("Undone"), "{undone:?}");
        assert_eq!(
            shell.goal_book.goal(1).unwrap().state,
            before,
            "{row_command}"
        );
        let mut queued = alloc::vec::Vec::new();
        loop {
            let next = goals_step(&mut shell, "next");
            if next == "ok none" {
                break;
            }
            queued.push(
                next.split_once(' ')
                    .unwrap()
                    .1
                    .split_once(' ')
                    .unwrap()
                    .1
                    .to_string(),
            );
        }
        // The row's own command, then the undo's: the last word to goals is
        // the state before.
        assert_eq!(
            queued.last().map(String::as_str),
            Some(back_command),
            "{queued:?}"
        );
        assert!(queued.iter().any(|c| c == row_command), "{queued:?}");
    }
}
