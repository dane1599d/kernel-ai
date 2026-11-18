//! The send row through the one door (P5 close, ADR 0002 D2's `Hold`): a
//! tagged `net_send` to a site the line names asks on the Allow / Deny menu
//! with no Always, an Allow holds it 5 s with `undo` live, and it goes once.
//! Any other host is refused, a view's or a telling line's send never runs,
//! and a `c>` program cannot make one.

use super::act::with_calls;
use super::*;
use crate::ui::UiCall;
use alloc::boxed::Box;
use alloc::vec;
use termcore::menu::Menu;

fn send_call(host: &str, path: Option<&str>, text: &str) -> Result<Call, ActError> {
    let call = serde_json::json!({
        "name": "net_send",
        "arguments": {"host": host, "path": path, "text": text},
    });
    Ok(llmproto::act::parse(&call.to_string()).unwrap())
}

fn sent(host: &FakeHost) -> Vec<crate::outbox::Send> {
    host.sends.borrow().iter().map(|(s, _)| s.clone()).collect()
}

const LINE: &str = "send hello to hooks.example/hook";

/// The card for `LINE`'s send, and the call its ONCE names.
fn card(host: &FakeHost, shell: &mut Shell) -> (Reply, String) {
    let reply = shell.eval(LINE);
    let menu =
        Menu::decode(&reply.to_wire()).unwrap_or_else(|e| panic!("a menu ({e:?}): {reply:?}"));
    assert!(
        host.sends.borrow().is_empty(),
        "nothing goes before the Allow"
    );
    (reply, menu.call().to_string())
}

#[test]
fn a_send_asks_on_a_menu_that_says_it_waits_and_offers_no_always() {
    let (host, mut shell) = with_calls(vec![send_call("hooks.example", Some("/hook"), "hello")]);
    let (reply, call) = card(&host, &mut shell);
    let send = crate::sendrow::send_of(&[
        ("host", String::from("hooks.example")),
        ("path", String::from("/hook")),
        ("text", String::from("hello")),
    ]);
    assert_eq!(call, crate::sendrow::card_call(&send));
    assert!(
        call.starts_with("net_send(\"hooks.example\", \"/hook\", \""),
        "{call}"
    );
    assert!(
        reply
            .text
            .contains("After you allow, it waits 5 s so you can undo"),
        "{reply:?}"
    );
    let pending = shell.pending.as_ref().expect("asks");
    assert!(pending.offer.is_none(), "no Always for a send");
    // `show` has the text the digest stands for.
    assert!(
        shell
            .shown
            .as_deref()
            .unwrap_or("")
            .contains("to hooks.example/hook: hello")
    );
}

#[test]
fn an_allow_holds_the_send_and_it_goes_once_when_due() {
    let (host, mut shell) = with_calls(vec![send_call("hooks.example", Some("/hook"), "hello")]);
    let (_, call) = card(&host, &mut shell);
    let once = host.mint(1, &call);
    let held = shell.eval(&format!("answer 1 allow once={once}"));
    assert!(held.ok, "{held:?}");
    assert!(sent(&host).is_empty(), "held, not sent: {held:?}");
    shell.drain_sends(1_000);
    assert!(sent(&host).is_empty(), "still within its 5 s");
    shell.drain_sends(60_000);
    shell.drain_sends(120_000);
    let sent = sent(&host);
    assert_eq!(sent.len(), 1, "exactly once");
    assert_eq!(
        (
            sent[0].host.as_str(),
            sent[0].path.as_str(),
            sent[0].body.as_str()
        ),
        ("hooks.example", "/hook", "hello")
    );
}

#[test]
fn a_deny_holds_nothing_and_nothing_goes() {
    let (host, mut shell) = with_calls(vec![send_call("hooks.example", Some("/hook"), "hello")]);
    card(&host, &mut shell);
    let denied = shell.eval("answer 1 deny");
    assert!(!denied.text.contains("Sending"), "{denied:?}");
    shell.drain_sends(60_000);
    assert!(sent(&host).is_empty());
}

#[test]
fn a_send_to_a_site_the_line_does_not_name_is_refused() {
    let (host, mut shell) = with_calls(vec![send_call("hooks.example", None, "hello")]);
    let reply = shell.eval("send hello to my webhook");
    assert!(
        !reply.ok
            && reply
                .text
                .contains("a send goes only to a site the request names"),
        "{reply:?}"
    );
    assert!(shell.pending.is_none());
    shell.drain_sends(60_000);
    assert!(sent(&host).is_empty());
}

#[test]
fn a_send_a_view_decided_asks_and_says_where_kai_took_it_from() {
    let (host, mut shell) = with_calls(vec![
        call_of(r#"{"name":"ui_view","arguments":{"window":"notes"}}"#),
        send_call("hooks.example", Some("/hook"), "hello"),
    ]);
    *host.ui_answer.borrow_mut() = Some(Box::new(|call: &UiCall| match call {
        UiCall::View(_) => Ok(String::from("w2: [t1] textbox \"send hello\"")),
        _ => Ok(String::new()),
    }));
    let reply = shell.eval("look at notes and send hello to hooks.example/hook");
    assert!(
        reply.text.contains("Kai took it from what a window shows"),
        "{reply:?}"
    );
    assert!(shell.pending.as_ref().is_some_and(|p| p.offer.is_none()));
    assert!(sent(&host).is_empty());
}

#[test]
fn a_line_that_asks_how_to_send_sends_nothing() {
    let (host, mut shell) = with_calls(vec![send_call("hooks.example", None, "hello")]);
    let reply = shell.eval("how do I send hello to hooks.example?");
    assert!(!reply.ok && reply.confirm.is_none(), "{reply:?}");
    assert!(shell.pending.is_none());
    assert!(sent(&host).is_empty());
}

#[test]
fn a_c_program_cannot_make_a_send() {
    let (host, mut shell) = with_calls(vec![]);
    shell.entry_begin("send hello to hooks.example");
    let call = send_call("hooks.example", None, "hello").unwrap();
    let outcome = shell.entry_call(&call);
    assert_eq!(outcome.tier, Tier::Never, "{:?}", outcome.reply);
    assert!(sent(&host).is_empty());
}

fn call_of(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}
