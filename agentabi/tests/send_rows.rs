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
    // (a) A line that asks how is told, never sent: refused, no card.
    assert_eq!(
        one("how do I send hello to hooks.example?", &plain),
        (Tier::Never, Why::SendUnasked)
    );
    // (b) A describe line names the send only to be told about it.
    assert_eq!(
        one("could you describe sending hello to hooks.example", &plain),
        (Tier::Never, Why::SendUnasked)
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

#[test]
fn a_question_a_view_steered_toward_a_send_is_refused() {
    // (c) After `ui_view`, the window chose the call: Steered, which skips the
    // shell's telling-line guard. The line asks for no send, so none.
    let viewed: Vec<Call<'static>> = send("hooks.example", Label::Literal, Label::Literal)
        .into_iter()
        .map(|call| Call {
            control: Label::Content("ui"),
            ..call
        })
        .collect();
    assert_eq!(
        one(
            "what does the notes window say about hooks.example?",
            &viewed
        ),
        (Tier::Never, Why::SendUnasked)
    );
    // A line that asks for the send, steered by a view, still asks.
    assert_eq!(
        one("look at notes and send hello to hooks.example", &viewed).0,
        Tier::Ask
    );
}

#[test]
fn a_table_word_never_stands_in_for_a_typed_host() {
    let wiki = send("en.wikipedia.org", Label::Literal, Label::Literal);
    // A table word after "to" names the destination; the host is not
    // written in the line, so the send asks as one the model filled in.
    assert_eq!(one("post hello to wikipedia", &wiki).0, Tier::Ask);
    assert_eq!(
        one("post hello to hooks.example, it's like wikipedia", &wiki),
        (Tier::Never, Why::SendHost)
    );
    assert_eq!(
        one("post hello about wikipedia", &wiki),
        (Tier::Never, Why::SendHost)
    );
}

#[test]
fn a_send_word_as_a_noun_or_in_a_telling_line_asks_for_no_send() {
    let plain = send("hooks.example", Label::Literal, Label::Literal);
    for line in [
        "check the notes window for the post about hooks.example",
        "the sender on hooks.example, what is it",
        "i wonder whether to send hello to hooks.example",
        "i know how to send hello to hooks.example",
        "what's sending to hooks.example for",
        "how's the posting to hooks.example going",
        "my latest post on hooks.example, read it",
        "a note about sending hello to hooks.example",
    ] {
        assert_eq!(one(line, &plain), (Tier::Never, Why::SendUnasked), "{line}");
    }
    for line in [
        "send hello to hooks.example",
        "can you post hello to hooks.example",
        "please submit hello to hooks.example",
    ] {
        assert_eq!(one(line, &plain).0, Tier::Ask, "{line}");
    }
}

#[test]
fn only_a_send_where_a_request_puts_its_verb_asks() {
    let plain = send("hooks.example", Label::Literal, Label::Literal);
    for line in [
        "check the blog post about hooks.example",
        "read the job posting on hooks.example",
        "if i send hello to hooks.example, what happens",
        "remind me how to send hello to hooks.example",
        "i want to know what sending hello to hooks.example does",
        "in case i send hello to hooks.example, keep a copy",
        "whether to send hello to hooks.example is up to you",
        "when you send hello to hooks.example it fails",
        "whats sending to hooks.example for",
        "hows posting to hooks.example going",
        "i know if i send hello to hooks.example it works",
    ] {
        assert_eq!(one(line, &plain), (Tier::Never, Why::SendUnasked), "{line}");
    }
    for line in [
        "post it to hooks.example",
        "could you send hello to hooks.example",
        "please post hello to hooks.example",
        "first, send hello to hooks.example",
        "resend hello to hooks.example",
        "i want to send hello to hooks.example",
        "look at notes and then send hello to hooks.example",
    ] {
        assert_eq!(one(line, &plain).0, Tier::Ask, "{line}");
    }
}

#[test]
fn the_final_probe_lines_ask_for_no_send() {
    let plain = send("hooks.example", Label::Literal, Label::Literal);
    for line in [
        "sending to hooks.example is safe?",
        "send hello to hooks.example, is that safe?",
        "i used to post hello to hooks.example",
        "it is easy to post hello to hooks.example",
        "a guide to posting on hooks.example",
        "the app also sends hello to hooks.example",
        "tom also sends hello to hooks.example",
        "and then sending hello to hooks.example, what next?",
        "i forgot to send hello to hooks.example yesterday",
        "how hard is it to send hello to hooks.example",
        "now sending to hooks.example works",
        "does it send hello to hooks.example?",
    ] {
        assert_eq!(one(line, &plain), (Tier::Never, Why::SendUnasked), "{line}");
    }
    for line in [
        "post it to hooks.example",
        "could you send hello to hooks.example?",
        "please post hello to hooks.example",
        "first, send hello to hooks.example",
        "resend hello to hooks.example",
        "i want to send hello to hooks.example",
        "look at notes and then send hello to hooks.example",
    ] {
        assert_eq!(one(line, &plain).0, Tier::Ask, "{line}");
    }
}
