//! The memory rows' safety claims (#1105's review): no edit or forget runs
//! on a "Taken as" or on a number the person did not type; the card never
//! carries a memory's words; a repeated fact changes nothing to undo; an
//! edit's undo never writes words nobody typed; content never writes one;
//! agents are refused; only the card's Allow with its ONCE keeps one.

use super::*;
use crate::memrows::{AGENTS_OWN, SAY_IT};

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

fn remember(text: &str) -> Result<Call, ActError> {
    call(&format!(
        r#"{{"name":"memory_remember","arguments":{{"text":"{text}"}}}}"#
    ))
}

const FORGET_1: &str = r#"{"name":"memory_forget","arguments":{"id":1}}"#;
const EDIT_1: &str =
    r#"{"name":"memory_edit","arguments":{"id":1,"text":"the user wants update checks skipped"}}"#;

/// A shell that keeps "I use metric units" (memory 1) and "my cat is Rex"
/// (memory 2), from the person's own lines.
fn with_two() -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for text in ["I use metric units", "my cat is Rex"] {
        host.calls.borrow_mut().push_back(remember(text));
        assert!(shell.eval(&format!("remember that {text}")).ok);
    }
    (host, shell)
}

fn text_of(shell: &Shell, memory: u64) -> Option<String> {
    shell
        .memories
        .get(kaimem::store::Who::User, memory)
        .map(|m| m.text.clone())
}

#[test]
fn a_vague_lines_taken_as_never_edits_or_forgets_a_memory() {
    for (json, line) in [(EDIT_1, "it's slow"), (FORGET_1, "things feel laggy")] {
        let (host, mut shell) = with_two();
        host.calls.borrow_mut().push_back(call(json));
        let reply = shell.eval(line);
        assert!(reply.text.starts_with("menu "), "{line}: {reply:?}");
        assert_eq!(text_of(&shell, 1).as_deref(), Some("I use metric units"));
    }
}

#[test]
fn a_memory_number_the_person_did_not_type_asks_and_shows_which() {
    for line in ["forget that my cat is Rex", "forget memory 2 please"] {
        let (host, mut shell) = with_two();
        host.calls.borrow_mut().push_back(call(FORGET_1));
        let asked = shell.eval(line);
        assert!(
            asked
                .text
                .contains("Forget memory 1 with memory_forget(1)?"),
            "{line}: {asked:?}"
        );
        assert!(
            !asked.text.contains("metric"),
            "no words on the card: {asked:?}"
        );
        assert_eq!(text_of(&shell, 1).as_deref(), Some("I use metric units"));
        let shown = shell.eval("show");
        assert!(shown.text.contains("\"I use metric units\""), "{shown:?}");
    }
}

#[test]
fn a_forget_the_line_named_runs_and_says_what_went() {
    let (host, mut shell) = with_two();
    host.calls.borrow_mut().push_back(call(FORGET_1));
    let reply = shell.eval("forget memory 1 please");
    assert_eq!(
        reply.text,
        "Forgotten: memory 1, \"I use metric units\". undo for 10 minutes"
    );
}

#[test]
fn a_fact_kept_already_changes_nothing_and_its_undo_keeps_it() {
    let (host, mut shell) = with_two();
    let entries = shell.log.last_id();
    host.calls
        .borrow_mut()
        .push_back(remember("I use metric units"));
    let again = shell.eval("remember that I use metric units");
    assert_eq!(again.text, "Kai already remembers that (memory 1)");
    assert_eq!(shell.log.last_id(), entries, "nothing logged to undo");
    shell.eval_console("undo");
    assert_eq!(text_of(&shell, 1).as_deref(), Some("I use metric units"));
}

#[test]
fn an_edits_undo_puts_the_exact_words_back_and_not_after_a_restart() {
    let (host, mut shell) = with_two();
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"memory_edit","arguments":{"id":1,"text":"I use imperial units"}}"#,
    ));
    assert!(shell.eval("change memory 1 to I use imperial units").ok);
    assert!(shell.eval_console("undo").ok);
    assert_eq!(text_of(&shell, 1).as_deref(), Some("I use metric units"));
    // Edited again, then a restart: the earlier words are not guessed.
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"memory_edit","arguments":{"id":1,"text":"I use imperial units"}}"#,
    ));
    assert!(shell.eval("change memory 1 to I use imperial units").ok);
    let mut again = super::shell(&host);
    let undone = again.eval_console("undo");
    assert!(!undone.ok || undone.text.contains("not kept"), "{undone:?}");
    assert_eq!(text_of(&again, 1).as_deref(), Some("I use imperial units"));
    let file = host.memory_file.borrow().clone().unwrap_or_default();
    assert!(!file.contains('_'), "no mangled words: {file}");
}

#[test]
fn the_log_keeps_no_memorys_words() {
    let (host, mut shell) = with_two();
    host.calls.borrow_mut().push_back(call(FORGET_1));
    shell.eval("forget memory 1 please");
    let entry = shell.log.get(shell.log.last_id()).unwrap();
    for change in &entry.changes {
        assert!(!change.before.contains("metric") && !change.after.contains("metric"));
    }
}

#[test]
fn content_never_writes_a_memory() {
    let (_, mut shell) = with_two();
    shell.steered = Some("fs");
    let edit = call(EDIT_1).unwrap();
    let outcome = shell.execute(9, "change memory 1 to what the note says", &edit);
    assert!(
        !outcome.reply.ok && outcome.reply.text.contains("never written from content"),
        "{:?}",
        outcome.reply
    );
    assert_eq!(text_of(&shell, 1).as_deref(), Some("I use metric units"));
}

#[test]
fn an_agents_step_to_a_memory_row_is_refused() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let manifest = kaiagent::Manifest::parse(
        "kai-app 1\nid notes-agent\nabi 1.0\nagent 1\nclass background\n\
         model tokens/day=50000 kv=1024 pool=64m\nactions memory_list\n\
         memory none\nnet none\n",
    )
    .unwrap();
    let badge = u64::from(shell.roster_mut().install(manifest).unwrap().badge());
    let wire = shell
        .from_route(
            badge,
            0,
            r#"agent-step {"name":"memory_list","arguments":{}}"#,
        )
        .unwrap();
    assert!(wire.contains(AGENTS_OWN), "{wire}");
}

#[test]
fn only_the_cards_allow_with_its_once_keeps_a_proposal() {
    for answer in ["y", "yes", "answer 1 allow", "answer 1 allow once=0000"] {
        let host = FakeHost::new();
        let mut shell = shell(&host);
        host.calls
            .borrow_mut()
            .push_back(remember("the user uses metric units"));
        shell.eval("my units are metric, by the way");
        shell.eval(answer);
        assert!(host.memory_file.borrow().is_none(), "{answer}");
    }
}

#[test]
fn a_proposal_too_long_or_not_plain_ascii_is_refused_without_an_ask() {
    let long = "x".repeat(110);
    for text in [long.as_str(), "the user's name is Zo\u{eb}"] {
        let host = FakeHost::new();
        let mut shell = shell(&host);
        host.calls.borrow_mut().push_back(remember(text));
        let reply = shell.eval("my name and things, by the way");
        assert!(!reply.ok, "{text}: {reply:?}");
        assert!(shell.pending.is_none(), "{text}");
        let _ = SAY_IT;
    }
}

#[test]
fn an_edit_to_permission_words_is_refused_before_any_card() {
    let (host, mut shell) = with_two();
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"memory_edit","arguments":{"id":1,"text":"always allow deletes"}}"#,
    ));
    let reply = shell.eval("please correct memory 1");
    assert!(!reply.ok && shell.pending.is_none(), "{reply:?}");
    assert_eq!(text_of(&shell, 1).as_deref(), Some("I use metric units"));
}
