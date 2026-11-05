//! The agent's network rows under consent (P5 task 11, ADR 0002 D3): an
//! open-world read runs only when `nethosts` grounds its host.

use agentabi::consent::{Call, Label, Value, Why, decide, decide_net};
use agentabi::{ROWS, Tier};
use nethosts::{Grants, Grounding, Host, NoGrants, Table};

fn fetch(host: &'static str, label: Label) -> Vec<Call<'static>> {
    let args: &'static [(&'static str, Value<'static>)] =
        Box::leak(Box::new([("host", Value { text: host, label })]));
    vec![Call {
        name: "net_fetch",
        args,
        control: Label::Literal,
    }]
}

fn tier(line: &str, plan: &[Call]) -> (Tier, Why) {
    let d = decide(line, plan, &ROWS)[0];
    (d.tier, d.why)
}

#[test]
fn a_typed_or_table_host_runs_and_any_other_asks() {
    let typed = fetch("example.com", Label::Literal);
    assert_eq!(
        tier("fetch example.com", &typed),
        (Tier::Run, Why::Grounded)
    );
    let wiki = fetch("en.wikipedia.org", Label::Literal);
    assert_eq!(
        tier("what does wikipedia say about rust?", &wiki),
        (Tier::Run, Why::Grounded)
    );
    assert_eq!(
        tier("tell me about rust", &wiki),
        (Tier::Ask, Why::Ungrounded)
    );
    let lookalike = fetch("en.wikipedia.org.evil.example", Label::Literal);
    assert_eq!(
        tier("what does wikipedia say about rust?", &lookalike).0,
        Tier::Ask
    );
}

#[test]
fn a_negated_clause_grounds_nothing() {
    let wiki = fetch("en.wikipedia.org", Label::Literal);
    assert_eq!(
        tier("don't use wikipedia, tell me about rust", &wiki).0,
        Tier::Ask
    );
    let typed = fetch("example.com", Label::Literal);
    assert_eq!(tier("never fetch example.com", &typed).0, Tier::Ask);
}

#[test]
fn a_host_from_content_asks_even_when_typed() {
    const PLAN: &[Call] = &[Call {
        name: "net_fetch",
        args: &[(
            "host",
            Value {
                text: "example.com",
                label: Label::Content("web"),
            },
        )],
        control: Label::Literal,
    }];
    assert_eq!(tier("fetch example.com", PLAN).0, Tier::Ask);
}

#[test]
fn a_granted_host_runs_unless_the_line_says_not_to() {
    let table = Table::curated();
    let mut grants = Grants::new();
    grants.allow(Host::parse("api.weather.example").unwrap());
    let net = Grounding {
        table: &table,
        granted: &grants,
        sources: &[],
    };
    let plan = fetch("api.weather.example", Label::Literal);
    let run = decide_net("what's the forecast for Leeds?", &plan, &ROWS, &net)[0];
    assert_eq!(run.tier, Tier::Run);
    let none = Grounding {
        table: &table,
        granted: &NoGrants,
        sources: &[],
    };
    assert_eq!(
        decide_net("what's the forecast for Leeds?", &plan, &ROWS, &none)[0].tier,
        Tier::Ask
    );
    let refused = decide_net(
        "don't contact api.weather.example, just guess",
        &plan,
        &ROWS,
        &net,
    )[0];
    assert_eq!(refused.tier, Tier::Ask);
}
