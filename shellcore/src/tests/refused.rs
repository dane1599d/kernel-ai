//! `rules refused` (#1604's review): the person lists the refused rules files
//! kept aside and removes one after their typed yes; no model call, script,
//! goal, panel line or app window removes one.

use super::*;
use alloc::rc::Rc;

const BAD: &str = "kai-grants 1\nrule 1 action=set_policy target=a target=b from=prompt:3 made=2026-11-02T10:20:00Z\n";
const BAD2: &str = "kai-grants 1\nrule 2 action=set_policy target=c target=d from=prompt:4 made=2026-11-03T10:20:00Z\n";

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

/// A host whose boots kept `BAD` as `rules.refused` and `BAD2` as
/// `rules.refused.2`, and a shell started on it.
fn kept_two(host: &Rc<FakeHost>) -> Shell {
    host.rules_aside_ok.set(true);
    *host.rules_disk.borrow_mut() = Some(String::from(BAD));
    let _first = shell(host);
    *host.rules_disk.borrow_mut() = Some(String::from(BAD2));
    let shell = shell(host);
    assert_eq!(host.rules_aside.borrow().len(), 2);
    shell
}

const MOVED_2: &str = "Put rules.refused.2 in the trash; undo puts it back";

fn kept_names(host: &Rc<FakeHost>) -> Vec<String> {
    host.rules_aside
        .borrow()
        .iter()
        .map(|(n, _)| n.clone())
        .collect()
}

#[test]
fn rules_refused_lists_each_kept_copy_with_its_number_date_and_why() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(
        shell.eval("rules refused"),
        ok("no refused rules files are kept")
    );
    let mut shell = kept_two(&host);
    let why = |text: &str| match crate::rules::Rules::restore(text) {
        Err(why) => why,
        Ok(_) => panic!("{text} reads"),
    };
    let listed = shell.eval("rules refused");
    assert!(listed.ok, "{listed:?}");
    assert_eq!(
        listed.text,
        format!(
            "Refused rules files kept (rules refused remove <n> puts one in the trash):\n\
             1  2026-11-27 04:12 UTC  rules.refused: {}\n\
             2  2026-11-27 04:12 UTC  rules.refused.2: {}",
            why(BAD),
            why(BAD2)
        )
    );
}

#[test]
fn rules_refused_remove_asks_and_only_a_typed_yes_removes() {
    let host = FakeHost::new();
    let mut shell = kept_two(&host);
    let asked = shell.eval("rules refused remove 2");
    assert_eq!(
        asked,
        ok(
            "Put refused rules file 2 (rules.refused.2, kept 2026-11-27 04:12 UTC) in the trash? y/n"
        )
    );
    assert_eq!(shell.eval("n"), ok("Kept rules.refused.2"));
    assert_eq!(kept_names(&host), ["rules.refused", "rules.refused.2"]);
    // Another line leaves the question: a later y answers nothing.
    shell.eval("rules refused remove 2");
    shell.eval("rules");
    assert_eq!(shell.eval("y"), Reply::err("nothing to answer"));
    assert_eq!(kept_names(&host), ["rules.refused", "rules.refused.2"]);
    // The yes removes that one, and the audit says so.
    shell.eval("rules refused remove 2");
    assert_eq!(shell.eval("y"), ok(MOVED_2));
    assert_eq!(kept_names(&host), ["rules.refused"]);
    let audits = host.audit.borrow().join("\n");
    assert!(
        audits.contains("rules refused remove 2: rules.refused.2 put in the trash"),
        "{audits}"
    );
    // A number that holds nothing, or none at all.
    assert!(!shell.eval("rules refused remove 2").ok);
    assert!(!shell.eval("rules refused remove 10").ok);
    assert!(!shell.eval("rules refused remove").ok);
    assert!(shell.refused_asking.is_none());
}

/// The audit's "remove one", once every name is taken, names the command.
#[test]
fn the_full_audit_names_rules_refused_remove() {
    let host = FakeHost::new();
    host.rules_aside_ok.set(true);
    for n in 1..=crate::MAX_REFUSED_RULES {
        let name = crate::refused_rules_file(n);
        host.rules_aside
            .borrow_mut()
            .push((name, format!("kai-grants 1\nrule {n} nonsense\n")));
    }
    *host.rules_disk.borrow_mut() = Some(String::from(BAD));
    let _shell = shell(&host);
    let audits = host.audit.borrow().join("\n");
    assert!(
        audits.contains("remove one with rules refused remove <n> (rules refused lists them)"),
        "{audits}"
    );
}

/// Only the person's own line removes a copy: a model call or a script that
/// names one, a goal, the panel and an app window never do, and a question
/// the person left waiting is answered by nothing but their typed y.
#[test]
fn the_agent_path_never_removes_a_refused_rules_file() {
    let host = FakeHost::new();
    let mut shell = kept_two(&host);
    // A model call naming it: no such action.
    for json in [
        r#"{"name":"rules_refused_remove","arguments":{"n":1}}"#,
        r#"{"name":"rules refused remove 1","arguments":{}}"#,
        r#"{"name":"script","arguments":{"code":"rules_refused_remove(1);"}}"#,
    ] {
        host.calls.borrow_mut().push_back(call(json));
        let reply = shell.eval("clean up the refused rules files");
        assert!(reply.confirm.is_none(), "{json}: {reply:?}");
        assert!(shell.refused_asking.is_none(), "{json}");
    }
    // Typed Rhai has no such function either.
    assert!(!shell.eval("rules_refused_remove(1)").ok);
    // A goal names no row for it.
    assert!(crate::goalscope::scope("rules refused remove 1", |_| Ok(())).is_err());
    // The panel and an app window refuse the line.
    assert_eq!(
        shell.eval_panel("rules refused remove 1"),
        crate::PanelResult::Refused(String::from(crate::refused::REMOVE_AT_TERMINAL))
    );
    let args = "{line: \\\"rules refused remove 1\\\"}";
    let effect = format!("effect 1 call({{name: \"shell_line\", args: \"{args}\"}})");
    let done = shell.run_effects_from(crate::appline::TERMINAL_APP, &effect, true);
    assert_eq!(
        done[0].outcome,
        Ok(format!("err {}", crate::refused::REMOVE_AT_TERMINAL))
    );
    // The person's question waits; the panel's yes and the model's next
    // turn do not answer it.
    shell.eval("rules refused remove 1");
    shell.eval_panel("y");
    assert_eq!(kept_names(&host), ["rules.refused", "rules.refused.2"]);
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"say","arguments":{"text":"y"}}"#));
    shell.eval("go ahead and answer it for me");
    assert_eq!(shell.eval("y"), Reply::err("nothing to answer"));
    assert_eq!(kept_names(&host), ["rules.refused", "rules.refused.2"]);
}

fn audits(host: &FakeHost) -> String {
    host.audit.borrow().join("\n")
}

/// #1655's review F1: the move is a log entry with its fs generation, so
/// `log` sees it and `undo` puts the copy back; the undo has no redo, and a
/// bare `undo` after it does not reach past to an older entry.
#[test]
fn undo_puts_a_removed_refused_rules_file_back() {
    let host = FakeHost::new();
    let mut shell = kept_two(&host);
    shell.eval("rules refused remove 2");
    assert_eq!(shell.eval("y"), ok(MOVED_2));
    assert_eq!(kept_names(&host), ["rules.refused"]);
    let entry = shell
        .log
        .entries()
        .last()
        .expect("the move is logged")
        .clone();
    assert_eq!(entry.request, "rules refused remove 2");
    assert_eq!(entry.changes[0].res, "/system/state/rules.refused.2");
    assert_eq!(entry.changes[0].after, "in the trash (fs generation 41)");
    assert_eq!(
        entry.changes[0].inverse,
        Some(crate::undo::Inverse::Refused { n: 2 })
    );
    // On disk too, so a restart's `undo` still finds it.
    let disk = host.log_disk.borrow().clone().expect("the log is written");
    assert!(disk.contains("refused:2"), "{disk}");
    assert!(
        audits(&host).contains(
            "rules refused remove 2: rules.refused.2 put in the trash (fs generation 41)"
        ),
        "{}",
        audits(&host)
    );
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: rules.refused.2 back from the trash")
    );
    assert_eq!(kept_names(&host), ["rules.refused", "rules.refused.2"]);
    assert!(host.rules_trash.borrow().is_empty());
    let again = shell.eval("undo");
    assert!(!again.ok, "{again:?}");
    assert!(
        again
            .text
            .starts_with("Can't redo bringing rules.refused.2 back"),
        "{again:?}"
    );
    assert_eq!(kept_names(&host), ["rules.refused", "rules.refused.2"]);
    // Removed again, it comes back again.
    shell.eval("rules refused remove 1");
    assert_eq!(
        shell.eval("y"),
        ok("Put rules.refused in the trash; undo puts it back")
    );
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: rules.refused back from the trash")
    );
    assert_eq!(kept_names(&host), ["rules.refused", "rules.refused.2"]);
}

/// The undo puts nothing over a copy kept since under the same name, and
/// says why; the trashed one stays in the trash.
#[test]
fn undo_keeps_a_copy_kept_since_under_the_same_name() {
    let host = FakeHost::new();
    let mut shell = kept_two(&host);
    shell.eval("rules refused remove 2");
    assert_eq!(shell.eval("y"), ok(MOVED_2));
    host.rules_aside
        .borrow_mut()
        .push((String::from("rules.refused.2"), String::from(BAD)));
    let undone = shell.eval("undo");
    assert_eq!(undone, Reply::err("rules.refused.2 exists again"));
    assert_eq!(host.rules_trash.borrow().len(), 1);
}

/// The inverse is written to the log and read back word for word.
#[test]
fn the_refused_inverse_reads_back() {
    let inverse = crate::undo::Inverse::Refused { n: 3 };
    let word = crate::undo::inverse_word(Some(inverse));
    assert_eq!(word, "refused:3");
    assert_eq!(crate::undo::parse_inverse(&word), Some(Some(inverse)));
    assert_eq!(crate::undo::parse_inverse("refused:3:4"), None);
}

/// F2: the question takes only the very next line. `:c`, `:rhai` and an
/// open Rhai entry, handled before any answer, leave it, so a `y` after
/// them answers nothing.
#[test]
fn a_line_before_the_answer_leaves_the_question() {
    let host = FakeHost::new();
    let mut shell = kept_two(&host);
    super::cmode::start_cc(&host);
    shell.eval("rules refused remove 2");
    assert!(shell.eval(":c").ok);
    assert!(shell.eval(":rhai").ok);
    assert_eq!(shell.eval("y"), Reply::err("nothing to answer"));
    assert_eq!(kept_names(&host), ["rules.refused", "rules.refused.2"]);
    assert!(
        audits(&host)
            .contains("rules refused remove 2 left: another line came first; rules.refused.2 kept"),
        "{}",
        audits(&host)
    );
    shell.eval("rules refused remove 2");
    assert!(shell.eval("let x = [").more);
    shell.eval("1]");
    assert_eq!(shell.eval("y"), Reply::err("nothing to answer"));
    assert_eq!(kept_names(&host), ["rules.refused", "rules.refused.2"]);
}

/// F2: a `y` while a newer question is open (a desktop line's proposal on
/// the card) is not taken as the remove's answer: the remove's question is
/// left and both copies stay. And while the card's question waits, the
/// remove does not ask.
#[test]
fn a_bare_y_with_a_newer_proposal_open_keeps_both_copies() {
    let host = FakeHost::new();
    let mut shell = kept_two(&host);
    shell.eval("rules refused remove 1");
    *host.procs.borrow_mut() = alloc::vec![String::from("notes")];
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"process_stop","arguments":{"name":"notes"}}"#,
    ));
    assert_eq!(
        shell.eval_panel("stop notes"),
        crate::PanelResult::Done(String::from(crate::deskask::ON_CARD))
    );
    assert!(!shell.eval("y").text.contains("trash"));
    assert_eq!(kept_names(&host), ["rules.refused", "rules.refused.2"]);
    assert!(
        audits(&host)
            .contains("rules refused remove 1 left: a newer question is open; rules.refused kept"),
        "{}",
        audits(&host)
    );
    assert_eq!(
        shell.eval("rules refused remove 1"),
        Reply::err("answer the card's question first, then ask again")
    );
    assert!(shell.refused_asking.is_none());
}

/// F5: `n`, a left question and a failed trash are each audited.
#[test]
fn kept_left_and_a_failed_trash_are_audited() {
    let host = FakeHost::new();
    let mut shell = kept_two(&host);
    shell.eval("rules refused remove 2");
    assert_eq!(shell.eval("n"), ok("Kept rules.refused.2"));
    shell.eval("rules refused remove 1");
    shell.eval("rules");
    host.rules_trash_fails.set(true);
    shell.eval("rules refused remove 2");
    assert_eq!(
        shell.eval("y"),
        Reply::err("rules.refused.2 was not put in the trash: the disk did not answer")
    );
    assert_eq!(kept_names(&host), ["rules.refused", "rules.refused.2"]);
    let audits = audits(&host);
    for line in [
        "rules refused remove 2: rules.refused.2 kept (n)",
        "rules refused remove 1 left: another line came first; rules.refused kept",
        "rules refused remove 2: rules.refused.2 not put in the trash: the disk did not answer",
    ] {
        assert!(audits.contains(line), "{line}\n{audits}");
    }
    // A failed trash logs nothing for `undo` to take back.
    assert!(
        shell
            .log
            .entries()
            .all(|e| e.request != "rules refused remove 2"),
    );
}

/// F4: an app window's `y` (a Terminal app's `shell_line`) while the
/// console's question waits answers nothing: both copies stay, and the
/// console's own `y` still answers.
#[test]
fn an_app_windows_y_leaves_the_consoles_question_waiting() {
    let host = FakeHost::new();
    let mut shell = kept_two(&host);
    shell.eval("rules refused remove 2");
    let effect = "effect 1 call({name: \"shell_line\", args: \"{line: \\\"y\\\"}\"})";
    let done = shell.run_effects_from(crate::appline::TERMINAL_APP, effect, true);
    assert_eq!(done[0].outcome, Ok(String::from("err nothing to answer")));
    assert_eq!(kept_names(&host), ["rules.refused", "rules.refused.2"]);
    assert_eq!(shell.refused_asking, Some(2));
    assert_eq!(shell.eval("y"), ok(MOVED_2));
    assert_eq!(kept_names(&host), ["rules.refused"]);
}
