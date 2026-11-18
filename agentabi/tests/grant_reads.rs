//! A read from a host only an app's grant grounds (ADR 0002 D3,
//! `consent_grant`): held so narrow it carries nothing the user did not type.
//! Each attack from the #961 security review asks.

use agentabi::consent::{Call, Label, UNKNOWN, Value, Why, consent_grant::SYSTEM, decide_net};
use agentabi::{ROWS, Tier};
use nethosts::{Granted, Grounding, Host, Table};

const WEATHER: &str = "api.weather.example";

/// `api.weather.example` granted to the app named here.
struct App(&'static str);

impl Granted for App {
    fn granted(&self, host: &Host) -> bool {
        host.as_str() == WEATHER
    }

    fn holders(&self, host: &Host) -> Vec<String> {
        match self.granted(host) {
            true => vec![String::from(self.0)],
            false => Vec::new(),
        }
    }
}

fn call(
    name: &'static str,
    host: &'static str,
    path: Option<Value<'static>>,
    control: Label,
) -> Call<'static> {
    let mut args = vec![(
        "host",
        Value {
            text: host,
            label: Label::Literal,
        },
    )];
    args.extend(path.map(|path| ("path", path)));
    Call {
        name,
        args: Box::leak(args.into_boxed_slice()),
        control,
    }
}

fn literal(text: &'static str) -> Option<Value<'static>> {
    Some(Value {
        text,
        label: Label::Literal,
    })
}

fn fetch(path: &'static str) -> Call<'static> {
    call("net_fetch", WEATHER, literal(path), Label::Literal)
}

fn decide(line: &str, plan: &[Call]) -> Vec<(Tier, Why)> {
    decide_for(&App("weather"), line, plan)
}

fn decide_for(app: &App, line: &str, plan: &[Call]) -> Vec<(Tier, Why)> {
    let table = Table::curated();
    let net = Grounding {
        table: &table,
        granted: app,
        sources: &[],
    };
    decide_net(line, plan, &ROWS, &net)
        .into_iter()
        .map(|d| (d.tier, d.why))
        .collect()
}

fn one(line: &str, plan: Call<'static>) -> (Tier, Why) {
    decide(line, &[plan])[0]
}

const LINE: &str = "what's the weather in Leeds?";

#[test]
fn a_read_the_line_asked_for_runs() {
    for path in ["", "/", "/leeds", "/weather/in/leeds"] {
        assert_eq!(
            one(LINE, fetch(path)),
            (Tier::Run, Why::GrantRead),
            "{path}"
        );
    }
    let bare = call("net_fetch", WEATHER, None, Label::Literal);
    assert_eq!(one(LINE, bare), (Tier::Run, Why::GrantRead));
    assert_eq!(
        one("look up the forecast", fetch("/forecast")),
        (Tier::Run, Why::GrantRead)
    );
}

#[test]
fn a_value_the_plan_does_not_know_is_no_path() {
    // `/q=${..}`, `"/" + fs_changes()`, `/${processes()}`: the script walk
    // knows no text for them.
    let unknown = call(
        "net_fetch",
        WEATHER,
        Some(Value {
            text: UNKNOWN,
            label: Label::Literal,
        }),
        Label::Literal,
    );
    assert_eq!(one(LINE, unknown), (Tier::Ask, Why::GrantPath));
    let system = call(
        "net_fetch",
        WEATHER,
        Some(Value {
            text: "/leeds",
            label: Label::System,
        }),
        Label::Literal,
    );
    assert_eq!(one(LINE, system), (Tier::Ask, Why::GrantPath));
}

#[test]
fn the_systems_state_deciding_a_read_steers_it() {
    let looped = call("net_fetch", WEATHER, literal("/leeds"), Label::System);
    assert_eq!(one(LINE, looped), (Tier::Ask, Why::GrantSteered(SYSTEM)));
    let viewed = call(
        "net_fetch",
        WEATHER,
        literal("/leeds"),
        Label::Content("ui"),
    );
    assert_eq!(one(LINE, viewed), (Tier::Ask, Why::GrantSteered("ui")));
}

#[test]
fn a_steered_read_from_a_typed_host_still_runs() {
    let viewed = call("net_fetch", WEATHER, literal("/x"), Label::Content("ui"));
    assert_eq!(
        one("fetch api.weather.example", viewed),
        (Tier::Run, Why::Grounded)
    );
}

#[test]
fn a_path_is_one_short_run_of_the_lines_own_words() {
    for path in [
        "/-.-.~--.-_..-!.-.$'()*+,;=:@",
        "/LeEdS/in/IN/the/WEATHER/in/leeds/leeds/in?the=IN&in=the",
        "/Leeds",
        "/leeds/leeds",
        "/leeds/weather",
        "/leeds?x=1",
        "/the/weather/in/leeds",
        "/leeds/",
        "//leeds",
        "leeds",
        "/what's",
        "/london",
    ] {
        // A digit in the path also takes the app's word away as a signal.
        assert_eq!(one(LINE, fetch(path)).0, Tier::Ask, "{path}");
    }
    // A pasted line goes out no more than three words at a time, and not at
    // all with no word that asks to go online.
    let pasted = "rewrite politely: tell Sam the Acme merger closes Friday at 40m";
    assert_eq!(
        one(
            pasted,
            fetch("/tell/sam/the/acme/merger/closes/friday/at/40m")
        )
        .0,
        Tier::Ask
    );
    assert_eq!(one(pasted, fetch("/acme/merger")).0, Tier::Ask);
}

#[test]
fn a_negated_word_is_no_path() {
    assert_eq!(
        one("what's the weather, not for leeds?", fetch("/leeds")),
        (Tier::Ask, Why::GrantPath)
    );
}

#[test]
fn a_line_that_asks_nothing_online_grounds_no_grant() {
    assert_eq!(
        one("my pin is 4471, add it to my notes", fetch("/?pin=4471")),
        (Tier::Ask, Why::Ungrounded)
    );
    assert_eq!(
        one("my pin is 4471, add it to my notes", fetch("/4471")),
        (Tier::Ask, Why::Ungrounded)
    );
    // The app's word counts only where it stands un-negated.
    assert_eq!(
        one("what is the weather like?", fetch("/")),
        (Tier::Run, Why::GrantRead),
        "the word asks for the app's site"
    );
    assert_eq!(
        one("not the weather, what's 2 + 2?", fetch("/")),
        (Tier::Ask, Why::Ungrounded)
    );
}

#[test]
fn a_line_that_says_to_stay_offline_bars_a_grant() {
    for line in [
        "don't go online, just guess the weather",
        "without looking it up, what's the weather?",
        "stay offline and guess the weather",
        "you can't use the web, what's the weather?",
        "never fetch anything, what's the weather?",
        "do not search, what's the weather?",
    ] {
        assert_eq!(
            one(line, fetch("/")),
            (Tier::Ask, Why::GrantOffline),
            "{line}"
        );
    }
    // A row's own cue counts too: `ping` is net_lookup's.
    let lookup = call("net_lookup", WEATHER, None, Label::Literal);
    assert_eq!(
        one("never ping anything, is the weather app working?", lookup),
        (Tier::Ask, Why::GrantOffline)
    );
}

#[test]
fn one_grant_read_a_plan() {
    let plan = [fetch("/leeds"), fetch("/in")];
    assert_eq!(
        decide(LINE, &plan),
        [(Tier::Ask, Why::GrantMany), (Tier::Ask, Why::GrantMany)]
    );
}

#[test]
fn a_grant_grounds_its_one_host() {
    for host in [
        "x.api.weather.example",
        "weather.example",
        "api.weather.example.evil",
    ] {
        let other = call("net_fetch", host, None, Label::Literal);
        assert_eq!(one(LINE, other), (Tier::Ask, Why::Ungrounded), "{host}");
    }
}

#[test]
fn data_to_a_granted_host_asks_as_outbound() {
    let sending = call(
        "net_fetch",
        WEATHER,
        Some(Value {
            text: "/buy-milk",
            label: Label::Content("note \"todo\""),
        }),
        Label::Literal,
    );
    assert_eq!(
        one(LINE, sending),
        (Tier::Ask, Why::Outbound("note \"todo\""))
    );
}

#[test]
fn every_way_of_saying_stay_offline_bars_a_grant() {
    for line in [
        "Don’t go online, just guess the weather",
        "stay off the internet, what's the weather?",
        "stay off-line, what's the weather?",
        "don't, whatever you do, go online. what's the weather?",
        "search online? no, just guess the weather",
        "network disabled, what's the weather?",
        "internet off, what's the weather?",
        "you shouldnt go online, what's the weather?",
        "no downloading, guess the weather",
    ] {
        assert_eq!(
            one(line, fetch("/")),
            (Tier::Ask, Why::GrantOffline),
            "{line}"
        );
    }
}

#[test]
fn a_word_that_only_looks_online_is_no_signal() {
    for line in [
        "my pin is 4471, back it up in my notes",
        "set it up for 4471",
        "it looks like 4471 to me",
        "turn the page to 4471",
        "my site code is 4471",
    ] {
        assert_eq!(
            one(line, fetch("/4471")),
            (Tier::Ask, Why::Ungrounded),
            "{line}"
        );
    }
    // An app's name stands for its site only when it is no common word.
    assert_eq!(
        decide_for(&App("my"), "my pin is 4471", &[fetch("/4471")])[0],
        (Tier::Ask, Why::Ungrounded)
    );
    assert_eq!(
        decide_for(&App("the"), "the code is 4471", &[fetch("/4471")])[0],
        (Tier::Ask, Why::Ungrounded)
    );
}

#[test]
fn an_online_word_counts_by_its_start() {
    assert_eq!(
        one("browsing for the weather in leeds", fetch("/leeds")),
        (Tier::Run, Why::GrantRead)
    );
    assert_eq!(
        decide_for(&App("radar"), "look it up for leeds", &[fetch("/leeds")])[0],
        (Tier::Run, Why::GrantRead)
    );
}

#[test]
fn a_path_takes_each_word_once_even_when_the_line_repeats_it() {
    assert_eq!(
        one("what's the weather in leeds leeds?", fetch("/leeds/leeds")),
        (Tier::Ask, Why::GrantPath)
    );
}

#[test]
fn a_line_that_says_to_stay_offline_wins_over_a_steered_read() {
    // A view decided the fetch, and the person's Always rule would waive a
    // steered ask: the line's own "don't go online" asks first.
    let viewed = call(
        "net_fetch",
        WEATHER,
        literal("/leeds"),
        Label::Content("ui"),
    );
    assert_eq!(
        one(
            "don't go online, just look at notes and guess the weather in Leeds",
            viewed
        ),
        (Tier::Ask, Why::GrantOffline)
    );
    let system = call("net_fetch", WEATHER, literal("/leeds"), Label::System);
    assert_eq!(
        one("don't go online, what's the weather in Leeds?", system),
        (Tier::Ask, Why::GrantOffline)
    );
}

#[test]
fn every_apostrophe_and_auxiliary_negates() {
    for mark in [
        '`', '\u{B4}', '\u{2032}', '\u{FF07}', '\u{2B9}', '\u{55A}', '\u{2019}',
    ] {
        let line = format!("Don{mark}t go online, guess the weather");
        assert_eq!(
            one(&line, fetch("/")),
            (Tier::Ask, Why::GrantOffline),
            "{line}"
        );
    }
    for line in [
        "dont go online, what's the weather in Leeds?",
        "DONT go online, guess the weather",
        "aint going online, what's the weather in Leeds?",
        "you shouldnt browse, what's the weather in Leeds?",
        "isnt it better offline? what's the weather",
    ] {
        assert_eq!(
            one(line, fetch("/")),
            (Tier::Ask, Why::GrantOffline),
            "{line}"
        );
    }
}

#[test]
fn words_that_mean_stay_off_the_network_bar_a_grant() {
    for line in [
        "abstain from the web, guess the weather",
        "avoid the internet, what's the weather in Leeds?",
        "avoiding the web today, what's the weather in Leeds?",
        "ban online lookups, guess the weather",
        "forget the internet, what's the weather in Leeds?",
        "ignore the internet, what's the weather in Leeds?",
        "refrain from going online, what's the weather in Leeds?",
        "stay away from the web, what's the weather in Leeds?",
        "online is forbidden, what's the weather in Leeds?",
        "online is prohibited here, what's the weather in Leeds?",
        "nah, no looking it up, what's the weather in Leeds?",
        "the wifi is down, what's the weather in Leeds?",
        "I'm disconnected, what's the weather in Leeds?",
        "I'm unplugged, what's the weather in Leeds?",
        "I'm in airplane mode, what's the weather in Leeds?",
        "I'm off-grid, what's the weather in Leeds?",
        "I said no going online. What's the weather in Leeds?",
        "Don't look it up. What's the weather in Leeds?",
    ] {
        assert_eq!(
            one(line, fetch("/")),
            (Tier::Ask, Why::GrantOffline),
            "{line}"
        );
    }
}

#[test]
fn a_negation_in_another_sentence_is_not_the_lines_offline() {
    // Pinned in the shell too, with and without the person's Always rule.
    assert_eq!(
        one(
            "I don't know the weather in Leeds. Look it up online.",
            fetch("/")
        ),
        (Tier::Run, Why::GrantRead)
    );
}

#[test]
fn an_apps_word_signals_only_a_told_line_with_no_digit() {
    for app in ["pin", "password", "diary", "bank"] {
        for (line, path) in [
            ("my pin is 4471, add it to my notes", "/pin/is/4471"),
            (
                "my password is hunter2, add it to my notes",
                "/password/is/hunter2",
            ),
            ("add to my diary: met sam at noon", "/met/sam/at"),
            ("what's my bank pin 4471?", "/bank/pin/4471"),
        ] {
            assert_eq!(
                decide_for(&App(app), line, &[fetch(path)])[0].0,
                Tier::Ask,
                "{app}: {line}"
            );
        }
    }
    assert_eq!(one(LINE, fetch("/leeds")), (Tier::Run, Why::GrantRead));
}

#[test]
fn a_noun_that_names_the_internet_is_no_signal() {
    for (line, path) in [
        ("my google password is hunter2", "/hunter2"),
        ("my webcam pin is 4471", "/4471"),
        ("the website code is 4471", "/4471"),
        ("the browser code is 4471", "/4471"),
        ("my downloads folder has pin 4471", "/4471"),
        ("the url code is 4471", "/4471"),
        ("the http code is 4471", "/4471"),
        ("my ping pong score is 4471", "/4471"),
    ] {
        assert_eq!(
            one(line, fetch(path)),
            (Tier::Ask, Why::Ungrounded),
            "{line}"
        );
    }
    for line in [
        "google the weather in leeds",
        "please fetch the weather in leeds",
    ] {
        assert_eq!(
            one(line, fetch("/leeds")),
            (Tier::Run, Why::GrantRead),
            "{line}"
        );
    }
}

#[test]
fn an_app_name_each_check_refuses_on_its_own() {
    // Too short; a common English word; a kind word; a leading word.
    for app in ["zq", "notes", "app", "our"] {
        let line = format!("what's {app}?");
        assert_eq!(
            decide_for(&App(app), &line, &[fetch("/")])[0],
            (Tier::Ask, Why::Ungrounded),
            "{app}"
        );
    }
    assert_eq!(
        decide_for(&App("radar"), "what's radar?", &[fetch("/")])[0],
        (Tier::Run, Why::GrantRead)
    );
}

#[test]
fn an_answer_after_any_question_bars_a_grant() {
    for line in [
        "Can you look it up online? No, guess the weather in Leeds",
        "Go online, or just guess? Nope. The weather in Leeds",
        "Look it up online, or guess? No. The weather in Leeds",
        "Go online, maybe? No. The weather in Leeds",
        "search online? no, just guess the weather",
    ] {
        assert_eq!(
            one(line, fetch("/")),
            (Tier::Ask, Why::GrantOffline),
            "{line}"
        );
    }
}
