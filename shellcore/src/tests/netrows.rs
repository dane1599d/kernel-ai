//! The agent's network rows through the one door (P5 task 11): a fetch from a
//! grounded host runs, any other asks, and a clause that says not to grounds
//! nothing (`agentabi::consent` with `nethosts`).

use super::act::with_calls;
use super::*;
use alloc::vec;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

fn fetch(host: &str) -> Result<Call, ActError> {
    call(&format!(
        r#"{{"name":"net_fetch","arguments":{{"host":"{host}","path":null}}}}"#
    ))
}

#[test]
fn a_fetch_from_a_host_the_line_types_runs_and_shows_its_text_as_data() {
    let (host, mut shell) = with_calls(vec![fetch("example.com")]);
    let reply = shell.eval("fetch example.com");
    assert!(reply.ok && reply.confirm.is_none(), "{reply:?}");
    assert!(reply.text.contains("the page at example.com"), "{reply:?}");
    assert_eq!(*host.net_seen.borrow(), ["net_fetch example.com"]);
}

#[test]
fn a_table_word_grounds_its_host_and_a_question_counts() {
    let (host, mut shell) = with_calls(vec![fetch("en.wikipedia.org")]);
    let reply = shell.eval("what does wikipedia say about rust?");
    assert!(reply.ok && reply.confirm.is_none(), "{reply:?}");
    assert_eq!(*host.net_seen.borrow(), ["net_fetch en.wikipedia.org"]);
}

#[test]
fn a_host_the_line_does_not_name_asks_and_nothing_is_fetched() {
    let (host, mut shell) = with_calls(vec![fetch("en.wikipedia.org")]);
    let reply = shell.eval("tell me about rust");
    assert_eq!(reply.confirm, Some(1), "{reply:?}");
    assert!(host.net_seen.borrow().is_empty());
}

#[test]
fn a_clause_that_says_not_to_grounds_nothing() {
    let (host, mut shell) = with_calls(vec![fetch("en.wikipedia.org")]);
    let reply = shell.eval("don't use wikipedia, just tell me about rust");
    assert_eq!(reply.confirm, Some(1), "{reply:?}");
    let (host2, mut shell2) = with_calls(vec![fetch("example.com")]);
    let typed = shell2.eval("never fetch example.com");
    assert_eq!(typed.confirm, Some(1), "{typed:?}");
    assert!(host.net_seen.borrow().is_empty() && host2.net_seen.borrow().is_empty());
}

#[test]
fn a_lookalike_host_matches_nothing() {
    for lookalike in [
        "en.wikipedia.org.evil.example",
        "wikipedia.org",
        "x.example.com",
    ] {
        let (host, mut shell) = with_calls(vec![fetch(lookalike)]);
        let reply = shell.eval("fetch example.com from wikipedia");
        assert_eq!(reply.confirm, Some(1), "{lookalike}: {reply:?}");
        assert!(host.net_seen.borrow().is_empty(), "{lookalike}");
    }
}

#[test]
fn a_lookup_is_a_read_row_on_a_grounded_host() {
    let (host, mut shell) = with_calls(vec![
        call(r#"{"name":"net_lookup","arguments":{"host":"example.com"}}"#),
        call(r#"{"name":"net_lookup","arguments":{"host":"example.org"}}"#),
    ]);
    let found = shell.eval("ping example.com");
    assert!(
        found.ok
            && found
                .text
                .contains("example.com: 93.184.215.14; answers in 12 ms"),
        "{found:?}"
    );
    let other = shell.eval("resolve example.com");
    assert_eq!(
        other.confirm,
        Some(2),
        "example.org was not named: {other:?}"
    );
    assert_eq!(
        *host.net_seen.borrow(),
        ["net_resolve example.com", "net_ping example.com"]
    );
}
