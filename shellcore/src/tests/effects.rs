//! An app's effects (P4): a row its package declares, local and undoable,
//! runs, is logged as the app's and undoes; an undeclared row, or one that
//! would ask, is refused back to the app.

use super::*;
use crate::effects::{EffectDone, MAX_EFFECTS};

const ROUND_ROBIN: &str = r#"effect 1 call({name: "set_policy", args: "{name: \"round-robin\"}"})"#;

/// A `files` app whose effects may call the file rows it uses, and no other.
const FILES: &str = "version 1
kai-actions 1
world app:files/files@1.0.0
effects fs_list fs_read fs_move
action files_open
export app:files/actions@1.0.0#open
doc Open a folder.
sig func(path: string)
arg path text max=255 role=target from-request
facts read-only=yes destructive=no idempotent=yes open-world=no
inverse none
end
";

/// A host where `files` and `monitor` (which declares no effects) are loaded.
fn host() -> Rc<FakeHost> {
    let host = FakeHost::new();
    let mut apps = host.apps.borrow_mut();
    apps.rows.load("files", FILES).unwrap();
    let monitor = FILES
        .replace("app:files/", "app:monitor/")
        .replace("files_open", "monitor_open")
        .replace(
            "effects fs_list fs_read fs_move
",
            "",
        );
    apps.rows.load("monitor", &monitor).unwrap();
    drop(apps);
    host
}

#[test]
fn an_undeclared_row_is_refused_even_at_the_run_tier() {
    let host = host();
    let mut shell = shell(&host);
    let done = shell.run_effects("monitor", ROUND_ROBIN);
    assert!(
        matches!(&done[0].outcome, Err(why) if why.contains("does not declare")),
        "{done:?}"
    );
    assert_eq!(host.policy.get(), None);
}

#[test]
fn an_app_effect_may_not_remove_a_package_nor_declare_that_it_may() {
    let host = host();
    let mut shell = shell(&host);
    let remove = r#"effect 1 call({name: "pkg_remove", args: "{name: \"notes\"}"})"#;
    assert!(shell.run_effects("files", remove)[0].outcome.is_err());
    let mut apps = crate::apps::AppRows::default();
    let greedy = FILES.replace("effects fs_list", "effects pkg_remove fs_list");
    let err = apps.load("files", &greedy).unwrap_err();
    assert!(err.contains("pkg_remove, which no app may"), "{err}");
}

#[test]
fn an_effect_that_would_ask_is_refused_and_not_run() {
    let host = FakeHost::new();
    *host.procs.borrow_mut() = alloc::vec![String::from("notes")];
    let mut shell = shell(&host);
    let stop = r#"effect 4 call({name: "process_stop", args: "{name: \"notes\"}"})"#;
    let done = shell.run_effects("files", stop);
    assert!(matches!(
        &done[..],
        [EffectDone {
            ticket: 4,
            outcome: Err(_)
        }]
    ));
    assert_eq!(*host.procs.borrow(), alloc::vec![String::from("notes")]);
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|line| line.starts_with("files's effect 4 refused"))
    );
}

#[test]
fn unknown_rows_bad_arguments_and_notes_come_back_as_results() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let lines = concat!(
        "effect 1 call({name: \"format_disk\", args: \"{}\"})\n",
        "effect 2 call({name: \"set_policy\", args: \"{name: \\\"fastest\\\"}\"})\n",
        "effect 3 notify(\"saved\")\n",
        "result ok\n",
    );
    let done = shell.run_effects("settings", lines);
    assert_eq!(done.len(), 3);
    assert!(done[0].outcome.is_err());
    assert!(done[1].outcome.is_err());
    assert_eq!(done[2].outcome, Ok(String::new()));
    assert_eq!(host.policy.get(), None);
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|line| line == "settings notes: saved")
    );
}

#[test]
fn effects_past_the_limit_are_refused() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let lines: Vec<String> = (0..=MAX_EFFECTS)
        .map(|ticket| format!("effect {ticket} notify(\"n\")"))
        .collect();
    let done = shell.run_effects("files", &lines.join("\n"));
    assert_eq!(done.len(), MAX_EFFECTS + 1);
    assert!(done[..MAX_EFFECTS].iter().all(|d| d.outcome.is_ok()));
    assert!(done[MAX_EFFECTS].outcome.is_err());
}

#[test]
fn a_file_read_comes_back_to_the_app_and_a_move_undoes() {
    let host = host();
    host.files
        .borrow_mut()
        .insert("notes.txt".into(), "milk".into());
    let mut shell = shell(&host);
    let list = r#"effect 1 call({name: "fs_list", args: "{path: \"\"}"})"#;
    let done = shell.run_effects("files", list);
    assert_eq!(done[0].outcome, Ok(String::from("notes.txt\n")));
    let read = r#"effect 2 call({name: "fs_read", args: "{path: \"notes.txt\"}"})"#;
    assert_eq!(
        shell.run_effects("files", read)[0].outcome,
        Ok(String::from("milk\n"))
    );
    let moved =
        r#"effect 3 call({name: "fs_move", args: "{from: \"notes.txt\", to: \"plan.txt\"}"})"#;
    let done = shell.run_effects("files", moved);
    assert!(
        matches!(&done[0].outcome, Ok(line) if line.ends_with(". undo")),
        "{done:?}"
    );
    assert!(host.files.borrow().contains_key("plan.txt"));
    assert_eq!(shell.eval("undo"), ok("Undone: files back as they were"));
    assert!(host.files.borrow().contains_key("notes.txt"));
}

#[test]
fn an_app_cannot_reach_the_package_door() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let install = r#"effect 1 call({name: "pkg_install", args: "{name: \"hello\"}"})"#;
    assert!(shell.run_effects("files", install)[0].outcome.is_err());
}

const LIST: &str = r#"effect 1 call({name: "fs_list", args: "{path: \"\"}"})"#;

fn grant_call(name: &str, app: &str, row: &str) -> Call {
    let json = format!(r#"{{"name":"{name}","arguments":{{"app":"{app}","row":"{row}"}}}}"#);
    llmproto::act::parse(&json).unwrap()
}

#[test]
fn a_withheld_row_is_refused_as_an_undeclared_one_until_undo_gives_it_back() {
    let host = host();
    host.calls
        .borrow_mut()
        .push_back(Ok(grant_call("grant_revoke", "files", "fs_list")));
    let mut shell = shell(&host);
    assert!(shell.run_effects("files", LIST)[0].outcome.is_ok());
    // The AI's call asks unless the line names the row as the row is named;
    // Settings' click is the user's own.
    let asked = shell.eval("revoke fs_list from files");
    let id = asked.confirm.expect("the AI's revoke asks");
    let once = host.mint(id, "grant_revoke(\"files\", \"fs_list\")");
    let reply = shell.eval(&format!("answer {id} allow once={once}"));
    assert!(reply.text.starts_with("Done"), "{reply:?}");
    assert_eq!(
        host.withheld_disk.borrow().as_deref(),
        Some("kai-withheld 1\nfiles fs_list\n")
    );
    // From the next effect on, with no restart.
    let refused = shell.run_effects("files", LIST);
    assert!(
        matches!(&refused[0].outcome, Err(why) if why == "files does not declare effects that call fs_list"),
        "{refused:?}"
    );
    assert_eq!(
        crate::grants::allowed(host.as_ref(), "files"),
        ["fs_read", "fs_move"]
    );
    assert!(shell.eval("undo").text.starts_with("Undone: "));
    assert!(shell.run_effects("files", LIST)[0].outcome.is_ok());
    assert_eq!(
        host.withheld_disk.borrow().as_deref(),
        Some("kai-withheld 1\n")
    );
}

#[test]
fn only_a_row_the_app_declares_can_be_revoked() {
    let host = host();
    host.calls
        .borrow_mut()
        .push_back(Ok(grant_call("grant_revoke", "files", "set_policy")));
    let mut shell = shell(&host);
    let reply = shell.eval("revoke set_policy from files");
    assert!(!reply.text.starts_with("Done"), "{reply:?}");
    assert_eq!(*host.withheld_disk.borrow(), None);
}

#[test]
fn giving_a_row_back_asks_first() {
    let host = host();
    *host.withheld_disk.borrow_mut() = Some("kai-withheld 1\nfiles fs_list\n".into());
    host.calls
        .borrow_mut()
        .push_back(Ok(grant_call("grant_restore", "files", "fs_list")));
    let mut shell = shell(&host);
    let reply = shell.eval("give files fs_list back");
    assert!(reply.confirm.is_some(), "{reply:?}");
    assert!(shell.run_effects("files", LIST)[0].outcome.is_err());
}

#[test]
fn the_withheld_rows_are_read_again_at_boot_and_a_bad_file_fails_closed() {
    let host = host();
    *host.withheld_disk.borrow_mut() = Some("kai-withheld 1\nfiles fs_list\n".into());
    let mut shell = shell(&host);
    assert!(shell.run_effects("files", LIST)[0].outcome.is_err());
    let host = self::host();
    *host.withheld_disk.borrow_mut() = Some("kai-withheld 1\nfiles ../x\n".into());
    host.calls
        .borrow_mut()
        .push_back(Ok(grant_call("grant_revoke", "files", "fs_read")));
    let mut shell = super::shell(&host);
    // Every declared row is refused, and nothing overwrites the file.
    assert!(shell.run_effects("files", LIST)[0].outcome.is_err());
    assert!(crate::grants::allowed(host.as_ref(), "files").is_empty());
    let asked = shell.eval("revoke fs_read from files");
    if let Some(id) = asked.confirm {
        let once = host.mint(id, "grant_revoke(\"files\", \"fs_read\")");
        let reply = shell.eval(&format!("answer {id} allow once={once}"));
        assert!(!reply.text.starts_with("Done"), "{reply:?}");
    }
    assert_eq!(
        host.withheld_disk.borrow().as_deref(),
        Some(
            "kai-withheld 1
files ../x
"
        )
    );
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|line| line.contains("withheld grants on disk are out of shape"))
    );
}
