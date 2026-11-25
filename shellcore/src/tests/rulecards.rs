//! Group E of the 11-23 bug hunt: what "Always allow" on a card writes, and
//! what a card's answer reports, when the rule cannot be kept.

use super::act::with_calls;
use super::apps::NOTES;
use super::ui::desktop;
use super::*;
use alloc::vec;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

fn audited(host: &FakeHost, text: &str) -> bool {
    host.audit.borrow().iter().any(|line| line.contains(text))
}

const STOP_BUILD3: &str = r#"{"name":"process_stop","arguments":{"name":"build-3"}}"#;

/// A rules file holding the most rules a shell keeps (`MAX_RULES`).
fn full_rules() -> String {
    let mut text = String::from("kai-grants 1\n");
    for n in 1..=crate::rules::MAX_RULES {
        text.push_str(&format!(
            "rule {n} action=set_policy target=p{n} from=prompt:{n} made=1970-01-01T00:00:00Z\n"
        ));
    }
    text
}

/// A shell booted on `rules` with `calls` to come.
fn booted(rules: &str, calls: Vec<Result<Call, ActError>>) -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    *host.rules_disk.borrow_mut() = Some(String::from(rules));
    host.calls.borrow_mut().extend(calls);
    let shell = shell(&host);
    (host, shell)
}

/// #9, #15: a window named with a space could be offered as a rule's target,
/// which no rules file can hold; while it was kept, no rules save landed.
#[test]
fn a_target_no_rules_file_can_hold_is_never_offered_as_a_rule() {
    let close = r#"{"name":"window_close","arguments":{"window":"text editor"}}"#;
    let (host, mut shell) = with_calls(vec![call(close)]);
    desktop(&host);
    let shown = "window_close(\"text editor\")";
    let reply = shell.eval("close the text editor");
    let wire = reply.to_wire();
    assert!(wire.ends_with(shown), "{reply:?}");
    assert!(wire.contains("window_close calls"), "{wire}");
    assert!(!wire.contains("on \"text editor\""), "{wire}");
    // Chosen anyway, it is refused, and nothing is kept that could not be saved.
    let once = host.mint(1, shown);
    let answered = shell.eval(&format!("answer 1 always-target once={once}"));
    assert!(answered.text.contains("not run"), "{answered:?}");
    assert!(shell.save_rules().is_ok());
}

/// #9: a removal that did not reach the disk said "it asks again", and the rule
/// came back after a restart.
#[test]
fn a_removal_the_disk_did_not_take_says_it_holds_for_this_boot_only() {
    let host = FakeHost::new();
    host.read_errors.borrow_mut().push("rules_read");
    host.calls.borrow_mut().push_back(call(STOP_BUILD3));
    host.procs.borrow_mut().push(String::from("build-3"));
    let mut shell = shell(&host);
    host.read_errors.borrow_mut().clear();
    shell.eval("stop build-3");
    let once = host.mint(1, "process_stop(\"build-3\")");
    let added = shell.eval(&format!("answer 1 always-kind once={once}"));
    // #1344's review: the add says it was not saved, too.
    assert!(
        added
            .text
            .contains("Rule #1 added for this boot only: the rules were not saved"),
        "{added:?}"
    );
    let removed = shell.eval("rules_remove 1");
    assert!(!removed.ok, "{removed:?}");
    assert!(!removed.text.contains("it asks again"), "{removed:?}");
    assert!(
        removed.text.contains("may come back after a restart"),
        "{removed:?}"
    );
}

/// #10: an "Always allow" for an installed app's row wrote a rule the next
/// boot could not read, and the whole file went with it.
#[test]
fn an_apps_row_offers_no_rule_and_one_on_disk_costs_no_other() {
    let clear = r#"{"name":"notes_clear","arguments":{}}"#;
    let (host, mut shell) = with_calls(vec![call(clear)]);
    host.apps.borrow_mut().rows.load("notes", NOTES).unwrap();
    let reply = shell.eval("clear my notes");
    assert!(reply.confirm.is_some(), "{reply:?}");
    assert!(!reply.to_wire().contains("notes_clear calls"), "{reply:?}");
    // A file that holds one (written before this fix) keeps the others.
    let (host, mut shell) = booted(
        "kai-grants 1
rule 1 action=set_policy target=- from=prompt:3 made=2026-11-02T10:20:00Z
rule 2 action=notes_clear target=- from=prompt:4 made=2026-11-02T10:21:00Z
",
        vec![],
    );
    let rules = shell.eval("rules");
    assert!(rules.text.contains("#1 set_policy"), "{rules:?}");
    assert!(!rules.text.contains("notes_clear"), "{rules:?}");
    assert!(audited(
        &host,
        "a rule on disk was set aside: rule 2 names no action"
    ));
}

const BAD_RULES: &str = "kai-grants 1\nrule 1 action=set_policy target=a target=b from=prompt:3 made=2026-11-02T10:20:00Z\n";

/// #1344's review: a rules file refused whole is kept aside once, so rules
/// save again on this boot and the next, and `rules` says where it went.
#[test]
fn a_rules_file_refused_whole_is_kept_aside_and_rules_save_again() {
    let (host, mut shell) = booted(BAD_RULES, vec![call(STOP_BUILD3)]);
    assert_eq!(host.rules_aside.borrow().as_deref(), Some(BAD_RULES));
    let listed = shell.eval("rules");
    assert!(
        listed
            .text
            .starts_with("The rules file could not be read back at start")
            && listed.text.contains("/system/state/rules-refused"),
        "{listed:?}"
    );
    host.procs.borrow_mut().push(String::from("build-3"));
    shell.eval("stop build-3");
    let once = host.mint(1, "process_stop(\"build-3\")");
    let added = shell.eval(&format!("answer 1 always-kind once={once}"));
    assert!(
        added.text.contains("Rule #1 added; `rules` lists it."),
        "{added:?}"
    );
    let saved = host.rules_disk.borrow().clone().unwrap();
    assert!(saved.contains("action=process_stop"), "{saved}");
    // The next boot reads the new file, and keeps the old one aside.
    let mut again = shell_again(&host);
    assert!(again.eval("rules").text.starts_with("#1 process_stop"));
    assert_eq!(host.rules_aside.borrow().as_deref(), Some(BAD_RULES));
}

/// #1354's review: a rules file refused on a later boot is kept beside the
/// one kept before, never over it, and `rules` names where this one went.
#[test]
fn a_second_refused_rules_file_is_kept_beside_the_first() {
    let (host, _shell) = booted(BAD_RULES, vec![]);
    let second = BAD_RULES.replace("rule 1", "rule 2");
    *host.rules_disk.borrow_mut() = Some(second.clone());
    let mut again = shell_again(&host);
    let listed = again.eval("rules");
    assert!(
        listed.text.contains("/system/state/rules-refused.2"),
        "{listed:?}"
    );
    assert_eq!(host.rules_aside.borrow().as_deref(), Some(second.as_str()));
}

/// #10: where a refused file cannot be kept aside, it is never written over,
/// as a lost write there would make the loss permanent.
#[test]
fn a_rules_file_refused_whole_that_cannot_be_kept_aside_is_not_written_over() {
    let host = FakeHost::new();
    *host.rules_disk.borrow_mut() = Some(String::from(BAD_RULES));
    host.rules_aside_fails.set(true);
    host.calls.borrow_mut().push_back(call(STOP_BUILD3));
    let mut shell = shell(&host);
    assert!(
        shell
            .eval("rules")
            .text
            .contains("rules are not saved this boot")
    );
    host.procs.borrow_mut().push(String::from("build-3"));
    shell.eval("stop build-3");
    let once = host.mint(1, "process_stop(\"build-3\")");
    shell.eval(&format!("answer 1 always-kind once={once}"));
    assert_eq!(host.rules_disk.borrow().as_deref(), Some(BAD_RULES));
}

/// A second shell on the same host, as after a restart.
fn shell_again(host: &Rc<FakeHost>) -> Shell {
    shell(host)
}

/// #1344's review: an "Always allow" answering a question that no longer
/// waits keeps no rule, so its node goes.
#[test]
fn an_always_for_a_question_no_longer_waiting_revokes_its_node() {
    let (host, mut shell) = with_calls(vec![]);
    let answered = shell.eval("answer 4 always-kind once=1 rule=5");
    assert_eq!(answered, Reply::err("nothing to confirm"));
    assert_eq!(*host.revoked.borrow(), [5]);
}

/// #11: an "Always allow" the shell then refused left the node the trusted
/// path minted for it, holding a kernel rule slot until a reboot.
#[test]
fn an_always_the_shell_does_not_keep_revokes_its_node() {
    let (host, mut shell) = booted(&full_rules(), vec![call(STOP_BUILD3)]);
    host.procs.borrow_mut().push(String::from("build-3"));
    shell.eval("stop build-3");
    let once = host.mint(1, "process_stop(\"build-3\")");
    let answered = shell.eval(&format!("answer 1 always-kind once={once} rule=7"));
    assert!(answered.text.contains("too many rules"), "{answered:?}");
    assert_eq!(*host.revoked.borrow(), [7]);
}

/// #11's sibling: an "Always allow" that came after the question expired.
#[test]
fn an_always_after_the_question_expired_revokes_its_node() {
    let (host, mut shell) = with_calls(vec![call(STOP_BUILD3)]);
    host.procs.borrow_mut().push(String::from("build-3"));
    shell.eval("stop build-3");
    host.clock.set(host.clock.get() + 200_000_000_000);
    let once = host.mint(1, "process_stop(\"build-3\")");
    let answered = shell.eval(&format!("answer 1 always-kind once={once} rule=9"));
    assert!(answered.text.contains("expired"), "{answered:?}");
    assert_eq!(*host.revoked.borrow(), [9]);
}

/// #17: a card's yes that the shell then refused (its rule not kept) was
/// reported to the panel as "Done.".
#[test]
fn a_refused_yes_on_a_panel_lines_card_is_not_reported_done() {
    const STOP: &str = r#"{"name":"process_stop","arguments":{"name":"notes"}}"#;
    let (host, mut shell) = booted(&full_rules(), vec![call(STOP)]);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    shell.eval_panel("stop notes");
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    let menu = termcore::menu::Menu::decode(&card.to_wire()).unwrap();
    let once = host.mint(menu.id, menu.call());
    shell.eval_console(&format!(
        "answer {} always-kind once={once} rule=9",
        menu.id
    ));
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
    let told = shell.take_card_result();
    assert!(matches!(&told, Some((false, _))), "{told:?}");
}
