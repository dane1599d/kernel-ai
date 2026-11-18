//! The first send row, `net_send` (P5 close, ADR 0002 D2's `Hold`): it always
//! asks, only to a site the line names, and it is in no script's row set.

use agentabi::consent::{Call, Label, Value, Why, decide, decide_net};
use agentabi::{HOLD_MS, Inverse, NET_SEND, ROWS, SCRIPT_CALLS, Tier, find};
use nethosts::{Grants, Grounding, Host, Table};

fn value(text: &'static str, label: Label) -> Value<'static> {
    Value { text, label }
}

/// `net_send(host, text)`, the host and the text with these labels.
fn send(host: &'static str, host_label: Label, text: Label) -> Vec<Call<'static>> {
    let args: &'static [(&'static str, Value<'static>)] = Box::leak(Box::new([
        ("host", value(host, host_label)),
        ("text", value("hello", text)),
    ]));
    vec![Call {
        name: "net_send",
        args,
        control: Label::Literal,
    }]
}

fn one(line: &str, plan: &[Call]) -> (Tier, Why) {
    let d = decide(line, plan, &ROWS)[0];
    (d.tier, d.why)
}

#[test]
fn the_send_row_is_held_and_asks() {
    assert_eq!(NET_SEND.inverse, Inverse::Hold { ms: HOLD_MS });
    assert_eq!(HOLD_MS, 5_000);
    assert_eq!(agentabi::derive(&NET_SEND), Tier::Ask);
    let plain = send("hooks.example", Label::Literal, Label::Literal);
    assert_eq!(
        one("send hello to hooks.example", &plain),
        (Tier::Ask, Why::NoUndo)
    );
}

#[test]
fn a_send_goes_only_to_a_site_the_line_names() {
    // The model's own pick.
    let picked = send("hooks.example", Label::Literal, Label::Literal);
    assert_eq!(
        one("send hello to my webhook", &picked),
        (Tier::Never, Why::SendHost)
    );
    // A host read content gave, even one the line also types.
    let read = send("hooks.example", Label::Content("web"), Label::Literal);
    assert_eq!(
        one("send hello to hooks.example", &read),
        (Tier::Never, Why::SendHost)
    );
    // A host only an app's grant grounds: it may be read from, never sent to.
    let table = Table::curated();
    let mut grants = Grants::new();
    grants.allow(Host::parse("api.weather.example").unwrap());
    let net = Grounding {
        table: &table,
        granted: &grants,
        sources: &[],
    };
    let granted = send("api.weather.example", Label::Literal, Label::Literal);
    let d = decide_net("post hello online", &granted, &ROWS, &net)[0];
    assert_eq!((d.tier, d.why), (Tier::Never, Why::SendHost));
}

#[test]
fn read_content_as_the_text_asks_and_names_its_source() {
    let note = send(
        "hooks.example",
        Label::Literal,
        Label::Content("note \"todo\""),
    );
    assert_eq!(
        one("send my todo note to hooks.example", &note),
        (Tier::Ask, Why::Steered("note \"todo\""))
    );
}

#[test]
fn a_negated_or_told_line_never_runs_a_send() {
    let plain = send("hooks.example", Label::Literal, Label::Literal);
    // A host the line names only to say no names no destination: refused.
    assert_eq!(
        one("don't send hello to hooks.example yet", &plain),
        (Tier::Never, Why::SendHost)
    );
    // A line that asks how is told, never sent (the shell refuses an added
    // change on a telling line, ADR 0002 D3).
    assert_eq!(
        one("how do I send hello to hooks.example?", &plain),
        (Tier::Ask, Why::Added)
    );
}

#[test]
fn no_script_can_call_a_send() {
    assert!(find(&SCRIPT_CALLS, "net_send").is_none());
    assert!(
        SCRIPT_CALLS
            .iter()
            .all(|row| !matches!(row.inverse, Inverse::Hold { .. })),
        "a held row is in the script set"
    );
    assert!(find(&agentabi::NET_ROWS, "net_send").is_none());
}
