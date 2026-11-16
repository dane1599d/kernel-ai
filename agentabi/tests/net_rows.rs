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

/// A fetch of `host` whose path carries a note's text: data going out.
fn sending_note(host: &'static str) -> Vec<Call<'static>> {
    let args: &'static [(&'static str, Value<'static>)] = Box::leak(Box::new([
        (
            "host",
            Value {
                text: host,
                label: Label::Literal,
            },
        ),
        (
            "path",
            Value {
                text: "/?q=buy milk",
                label: Label::Content("note \"todo\""),
            },
        ),
    ]));
    vec![Call {
        name: "net_fetch",
        args,
        control: Label::Literal,
    }]
}

#[test]
fn read_content_goes_out_only_to_a_host_the_line_names() {
    // P-F (ADR 0002 D6): the line names the destination, typed or by a table word.
    let typed = sending_note("x.example");
    assert_eq!(
        tier("send my todo note to x.example", &typed),
        (Tier::Run, Why::Grounded)
    );
    let wiki = sending_note("en.wikipedia.org");
    assert_eq!(
        tier("search wikipedia for my todo note", &wiki),
        (Tier::Run, Why::Grounded)
    );
    // Any other host asks, and the ask names where the data came from.
    assert_eq!(
        tier("read my todo note", &typed),
        (Tier::Ask, Why::Outbound("note \"todo\""))
    );
    assert_eq!(
        tier("don't send my todo note to x.example", &typed),
        (Tier::Ask, Why::Outbound("note \"todo\""))
    );
}

#[test]
fn a_grant_grounds_a_fetch_but_names_no_destination_for_data() {
    let table = Table::curated();
    let mut grants = Grants::new();
    grants.allow(Host::parse("x.example").unwrap());
    let net = Grounding {
        table: &table,
        granted: &grants,
        sources: &[],
    };
    let plain = fetch("x.example", Label::Literal);
    assert_eq!(
        decide_net("check the status page", &plain, &ROWS, &net)[0].tier,
        Tier::Run
    );
    let sending = sending_note("x.example");
    let asked = decide_net("read my todo note", &sending, &ROWS, &net)[0];
    assert_eq!(asked.tier, Tier::Ask);
    assert_eq!(asked.why, Why::Outbound("note \"todo\""));
}

/// The grounding with `api.weather.example` granted to an app.
fn granted<R>(decide: impl FnOnce(&Grounding) -> R) -> R {
    let table = Table::curated();
    let mut grants = Grants::new();
    grants.allow(Host::parse("api.weather.example").unwrap());
    decide(&Grounding {
        table: &table,
        granted: &grants,
        sources: &[],
    })
}

/// A fetch of `host` at `path`, both written by the model, under `control`.
fn fetch_path(host: &'static str, path: &'static str, control: Label) -> Vec<Call<'static>> {
    let args: &'static [(&'static str, Value<'static>)] = Box::leak(Box::new([
        (
            "host",
            Value {
                text: host,
                label: Label::Literal,
            },
        ),
        (
            "path",
            Value {
                text: path,
                label: Label::Literal,
            },
        ),
    ]));
    vec![Call {
        name: "net_fetch",
        args,
        control,
    }]
}

fn granted_tier(line: &str, plan: &[Call]) -> (Tier, Why) {
    granted(|net| {
        let d = decide_net(line, plan, &ROWS, net)[0];
        (d.tier, d.why)
    })
}

#[test]
fn a_steered_call_is_not_grounded_by_a_grant() {
    // A second round after a view, or a `c>` entry after it read content: the
    // view decided this fetch, and only a grant grounds its host.
    let steered = fetch_path("api.weather.example", "/", Label::Content("ui"));
    assert_eq!(
        granted_tier("what's the weather in Leeds?", &steered),
        (Tier::Ask, Why::GrantSteered("ui"))
    );
}

#[test]
fn a_steered_call_to_a_typed_host_still_runs() {
    let steered = fetch_path("api.weather.example", "/", Label::Content("ui"));
    assert_eq!(
        granted_tier("fetch api.weather.example", &steered),
        (Tier::Run, Why::Grounded)
    );
}

#[test]
fn a_granted_host_takes_only_a_path_the_line_holds() {
    // An app's window title in the State line ("kai: check
    // api.weather.example/?s=...") can steer a first-round call to the app's
    // own granted host, carrying the selection in the path: it asks.
    let carried = fetch_path("api.weather.example", "/?s=meeting at 3", Label::Literal);
    assert_eq!(
        granted_tier("what's the weather in Leeds?", &carried),
        (Tier::Ask, Why::GrantPath)
    );
    let leeds = fetch_path("api.weather.example", "/leeds", Label::Literal);
    assert_eq!(
        granted_tier("what's the weather in Leeds?", &leeds),
        (Tier::Run, Why::Grounded)
    );
    let bare = fetch_path("api.weather.example", "/", Label::Literal);
    assert_eq!(
        granted_tier("what's the weather?", &bare),
        (Tier::Run, Why::Grounded)
    );
    // A typed host is the user's own choice of destination: its path is not
    // held to the line.
    assert_eq!(
        granted_tier("fetch api.weather.example", &carried),
        (Tier::Run, Why::Grounded)
    );
}

#[test]
fn a_line_that_says_to_stay_offline_bars_a_grant() {
    let plain = fetch("api.weather.example", Label::Literal);
    for line in [
        "don't go online, just guess the weather",
        "without the internet, what's the weather likely to be?",
        "never fetch anything, what's the weather?",
    ] {
        assert_eq!(
            granted_tier(line, &plain),
            (Tier::Ask, Why::Ungrounded),
            "{line}"
        );
    }
    assert_eq!(
        granted_tier("what's the weather? don't guess", &plain),
        (Tier::Run, Why::Grounded)
    );
}

#[test]
fn a_bait_host_still_asks_with_a_grant_on_file() {
    // A grant grounds its one host: a page's or the model's other host asks.
    let bait = fetch("weather-now.example", Label::Literal);
    assert_eq!(
        granted_tier("what's the weather in Leeds?", &bait),
        (Tier::Ask, Why::Ungrounded)
    );
    let under = fetch("x.api.weather.example", Label::Literal);
    assert_eq!(
        granted_tier("what's the weather in Leeds?", &under),
        (Tier::Ask, Why::Ungrounded)
    );
}

#[test]
fn data_to_a_granted_host_asks() {
    let sending = sending_note("api.weather.example");
    assert_eq!(
        granted_tier("what's the weather in Leeds?", &sending),
        (Tier::Ask, Why::Outbound("note \"todo\""))
    );
}
