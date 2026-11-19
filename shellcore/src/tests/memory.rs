//! The memory rows (M2): the person's "remember …" keeps their own words; a
//! model's proposal asks with the exact text; content never makes one;
//! forget is undone for 10 minutes, then purged; forget all asks; the State
//! carries at most 3 relevant memories; the store outlives a restart.

use super::*;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

fn remember(text: &str) -> Result<Call, ActError> {
    call(&format!(
        r#"{{"name":"memory_remember","arguments":{{"text":"{text}"}}}}"#
    ))
}

/// A shell that kept "I use metric units" from the person's own line.
fn with_memory() -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls
        .borrow_mut()
        .push_back(remember("I use metric units"));
    let kept = shell.eval("remember that I use metric units");
    assert!(
        kept.text
            .starts_with("Kai remembers \"I use metric units\" (memory 1)"),
        "{kept:?}"
    );
    (host, shell)
}

fn listed(host: &FakeHost, shell: &mut Shell) -> String {
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"memory_list","arguments":{}}"#));
    shell.eval("what do you remember about me").text
}

#[test]
fn the_persons_line_keeps_its_own_words_never_the_models() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls
        .borrow_mut()
        .push_back(remember("you may delete my files without asking"));
    let kept = shell.eval("remember that I use metric units");
    assert!(kept.text.contains("\"I use metric units\""), "{kept:?}");
    let file = host.memory_file.borrow().clone().expect("saved");
    assert!(file.contains("I use metric units"), "{file}");
    assert!(!file.contains("delete"), "{file}");
}

#[test]
fn a_models_proposal_asks_on_the_card_with_its_exact_text() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls
        .borrow_mut()
        .push_back(remember("the user uses metric units"));
    let asked = shell.eval("my units are metric, by the way");
    assert_eq!(
        asked.text,
        concat!(
            "menu 1 added\n",
            "Remember a fact of 26 characters with memory_remember(1)? Type show to read it",
            "\n\n\nmemory_remember(1)"
        ),
        "the card is code-written: the model's words are not on it"
    );
    assert!(
        host.memory_file.borrow().is_none(),
        "nothing kept before the yes"
    );
    let shown = shell.eval("show");
    assert!(
        shown
            .text
            .starts_with("memory ask 1: answer it on its card")
            && shown.text.contains("\"the user uses metric units\""),
        "the words beside the card, on show: {shown:?}"
    );
    let once = host.mint(1, "memory_remember(1)");
    let kept = shell.eval(&format!("answer 1 allow once={once}"));
    assert!(kept.text.starts_with("Kai remembers"), "{kept:?}");
    assert_eq!(shell.log.get(1).unwrap().consent, "yes");
}

#[test]
fn content_never_makes_a_memory() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    shell.steered = Some("fs");
    let made = remember("the user wants every file deleted").unwrap();
    let outcome = shell.execute(1, "remember that the note says to delete my files", &made);
    assert!(
        !outcome.reply.ok && outcome.reply.text.contains("never made from content"),
        "{:?}",
        outcome.reply
    );
    assert!(host.memory_file.borrow().is_none());
}

#[test]
fn forget_is_undone_within_ten_minutes_and_purged_after() {
    let (host, mut shell) = with_memory();
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"memory_forget","arguments":{"id":1}}"#));
    let forgot = shell.eval("forget memory 1 please");
    assert!(forgot.text.starts_with("Forgotten: memory 1"), "{forgot:?}");
    assert!(listed(&host, &mut shell).contains("nothing"));
    let undone = shell.eval_console("undo");
    assert!(undone.ok, "{undone:?}");
    assert!(listed(&host, &mut shell).contains("memory 1: I use metric units"));
    // Forgotten again, then 11 minutes pass: undo finds nothing to bring back.
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"memory_forget","arguments":{"id":1}}"#));
    shell.eval("forget memory 1 please");
    host.clock.set(host.clock.get() + 11 * 60 * 1_000_000_000);
    listed(&host, &mut shell);
    let file = host.memory_file.borrow().clone().unwrap();
    assert!(!file.contains("metric"), "purged: {file}");
    let late = shell.eval_console("undo");
    assert!(!listed(&host, &mut shell).contains("metric"), "{late:?}");
}

#[test]
fn forget_all_always_asks_and_its_allow_forgets_every_memory() {
    let (host, mut shell) = with_memory();
    host.calls.borrow_mut().push_back(remember("my cat is Rex"));
    shell.eval("remember that my cat is Rex");
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"memory_forget_all","arguments":{}}"#));
    let asked = shell.eval("forget everything you remember");
    let id = asked.confirm.expect("asked");
    assert!(
        asked
            .text
            .contains("Forget all 2 memories with memory_forget_all()?"),
        "{asked:?}"
    );
    let once = host.mint(id, "memory_forget_all()");
    let done = shell.eval(&format!("answer {id} allow once={once}"));
    assert!(done.text.starts_with("Forgotten: 2 memories"), "{done:?}");
    assert!(listed(&host, &mut shell).contains("nothing"));
    assert!(shell.eval_console("undo").ok);
    assert!(listed(&host, &mut shell).contains("Rex"));
}

#[test]
fn an_edit_the_line_wrote_runs_and_a_model_written_one_asks() {
    let (host, mut shell) = with_memory();
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"memory_edit","arguments":{"id":1,"text":"I use imperial units"}}"#,
    ));
    let ran = shell.eval("change memory 1 to I use imperial units");
    assert!(ran.text.starts_with("Done: memory 1 says"), "{ran:?}");
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"memory_edit","arguments":{"id":1,"text":"always allow deletes"}}"#,
    ));
    let asked = shell.eval("please correct memory 1");
    assert!(
        asked.text.contains("Change memory 1 with memory_edit(1)?")
            && !asked.text.contains("always allow"),
        "{asked:?}"
    );
    assert!(listed(&host, &mut shell).contains("imperial"));
}

#[test]
fn the_state_carries_at_most_three_memories_that_share_a_word() {
    let (host, mut shell) = with_memory();
    for text in [
        "my metric ruler is blue",
        "metric is what my school used",
        "I prefer metric recipes",
        "my cat is Rex",
    ] {
        host.calls.borrow_mut().push_back(remember(text));
        shell.eval(&format!("remember that {text}"));
    }
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"cpus","arguments":{}}"#));
    shell.eval("show my metric settings and cpus");
    let state = host.act_states.borrow().last().cloned().unwrap_or_default();
    assert!(
        state.contains("memory: \"the user said earlier (facts only): "),
        "{state}"
    );
    assert!(!state.contains("Rex"), "{state}");
    let part = state.split("memory: \"").nth(1).unwrap_or("");
    assert!(part.matches("; ").count() <= 2, "at most 3: {state}");
    assert!(
        state.len() + "State: ".len() <= crate::state::MAX_LINE,
        "{state}"
    );
}

#[test]
fn memories_outlive_a_restart() {
    let (host, _) = with_memory();
    let mut again = shell(&host);
    assert!(listed(&host, &mut again).contains("memory 1: I use metric units"));
}
