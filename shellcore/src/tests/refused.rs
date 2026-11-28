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
    assert_eq!(shell.eval("y"), ok("Put rules.refused.2 in the trash"));
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
