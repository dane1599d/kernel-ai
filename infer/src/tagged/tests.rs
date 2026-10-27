//! Every path of the tagged grammar, and the engine under it: the same answer
//! from a warm cache and a fresh one on every tier, with jump-forward and token
//! healing, and always a closed call that parses.

extern crate std;

use super::*;
use crate::cache_tests::assert_cache_holds_its_tokens;
use crate::chat::{Budget, Engine, Role, Turn};
use crate::fixture::{self, Spec};
use crate::{Next, Sampler};
use alloc::format;
use alloc::string::String;
use quant::Tier;

const POLICY: Kind = Kind::Enum(&["interactive", "round-robin"]);
const NAME: Kind = Kind::Text {
    chars: 32,
    lines: 1,
    line_chars: 32,
    empty: false,
};
const SAY: Kind = Kind::Text {
    chars: 240,
    lines: 1,
    line_chars: 240,
    empty: true,
};
const CODE: Kind = Kind::Text {
    chars: 3072,
    lines: 40,
    line_chars: 120,
    empty: true,
};
const OPTION: Kind = Kind::Text {
    chars: 40,
    lines: 1,
    line_chars: 40,
    empty: false,
};
const LINE: Kind = Kind::Int {
    min: 1,
    max: 99_999,
};

fn arg(name: &'static str, kind: Kind<'static>) -> Arg<'static> {
    Arg { name, kind }
}

fn actions() -> Vec<Action<'static>> {
    let leak = |args: Vec<Arg<'static>>| -> &'static [Arg<'static>] { args.leak() };
    let mut all = alloc::vec![
        Action {
            name: "cpus",
            args: &[]
        },
        Action {
            name: "set_policy",
            args: leak(alloc::vec![arg("name", POLICY)]),
        },
        Action {
            name: "undo",
            args: leak(alloc::vec![arg("steps", Kind::Int { min: 1, max: 9 })]),
        },
        Action {
            name: "editor_open",
            args: leak(alloc::vec![
                arg("path", NAME),
                arg("line", Kind::Opt(&LINE))
            ]),
        },
        Action {
            name: "net_ping",
            args: leak(alloc::vec![arg("host", Kind::Host)]),
        },
        Action {
            name: "say",
            args: leak(alloc::vec![arg("text", SAY)]),
        },
        Action {
            name: "clarify",
            args: leak(alloc::vec![
                arg("question", OPTION),
                arg("options", Kind::List(&OPTION, 6)),
            ]),
        },
        Action {
            name: "script",
            args: leak(alloc::vec![
                arg("code", CODE),
                arg("lang", Kind::Opt(&Kind::Enum(&["rhai"]))),
            ]),
        },
    ];
    // Past the 64 words a `Choice` takes, with shared prefixes.
    for i in 0..70 {
        let name: &'static str = String::leak(format!("stub_{i}"));
        all.push(Action { name, args: &[] });
    }
    all
}

fn takes(g: &Tagged<'_>, text: &str) -> Result<State, Stop> {
    g.advance(g.start(), text.as_bytes())
}

fn whole(g: &Tagged<'_>, text: &str) -> bool {
    takes(g, text).is_ok_and(|s| g.is_done(&s))
}

#[test]
fn calls_of_every_shape_parse_and_close() {
    let acts = actions();
    let g = Tagged::new(&acts).unwrap();
    for text in [
        r#"{"name":"cpus","arguments":{}}"#,
        r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#,
        r#"{"name":"undo","arguments":{"steps":3}}"#,
        r#"{"name":"editor_open","arguments":{"path":"/home/a.rs","line":null}}"#,
        r#"{"name":"editor_open","arguments":{"path":"/home/a.rs","line":120}}"#,
        r#"{"name":"clarify","arguments":{"question":"Which?","options":["a","b c"]}}"#,
        r#"{"name":"clarify","arguments":{"question":"Which?","options":[]}}"#,
        r#"{"name":"script","arguments":{"code":"let a = 1;\nprint(\"a\" + a);","lang":null}}"#,
        r#"{"name":"script","arguments":{"code":"","lang":"rhai"}}"#,
        r#"{"name":"net_ping","arguments":{"host":"example.com"}}"#,
        r#"{"name":"say","arguments":{"text":""}}"#,
        r#"{"name":"stub_7","arguments":{}}"#,
        r#"{"name":"stub_69","arguments":{}}"#,
    ] {
        assert!(whole(&g, text), "{text}");
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert!(parsed["name"].is_string() && parsed["arguments"].is_object());
    }
    let s = takes(&g, r#"{"name":"undo""#).unwrap();
    assert_eq!(g.action(&s), Some("undo"));
    assert_eq!(g.action(&takes(&g, r#"{"name":"und"#).unwrap()), None);
}

#[test]
fn the_mask_refuses_what_the_action_cannot_take() {
    let acts = actions();
    let g = Tagged::new(&acts).unwrap();
    for text in [
        r#"{"name":"cpux"#,
        r#"{"name":"stub_70"#,
        r#"{"name":"set_policy","arguments":{"name":"fifo"#,
        r#"{"name":"undo","arguments":{"steps":10"#,
        r#"{"name":"undo","arguments":{"steps":0"#,
        r#"{"name":"undo","arguments":{"steps":-"#,
        r#"{"name":"undo","arguments":{"steps":"1"#,
        r#"{"name":"net_ping","arguments":{"host":"Example"#,
        r#"{"name":"net_ping","arguments":{"host":"""#,
        r#"{"name":"editor_open","arguments":{"path":"""#,
        r#"{"name":"say","arguments":{"text":"a\n"#,
        r#"{"name":"say","arguments":{"text":"a\t"#,
        r#"{"name":"script","arguments":{"code":"","lang":"c"#,
        r#"{"name":"editor_open","arguments":{"path":"/a","line":nul,"#,
        r#"{"name":"cpus","arguments":{} }"#,
        r#"{"name":"cpus","arguments":{}}}"#,
        r#" {"#,
    ] {
        assert_eq!(takes(&g, text), Err(Stop::Masked), "{text}");
    }
    let say = |n: usize| format!(r#"{{"name":"say","arguments":{{"text":"{}"#, "a".repeat(n));
    assert_eq!(takes(&g, &say(241)), Err(Stop::Masked));
    // At the limit an escape could make no character: only the quote may come.
    let at = takes(&g, &say(240)).unwrap();
    assert_eq!(g.step(at, b'\\'), Err(Stop::Masked));
    assert!(g.step(at, b'"').is_ok());
}

#[test]
fn code_holds_lines_within_its_limits() {
    let acts = actions();
    let g = Tagged::new(&acts).unwrap();
    let code = |body: &str| format!(r#"{{"name":"script","arguments":{{"code":"{body}"#);
    let forty = alloc::vec!["x;"; 40].join("\\n");
    assert!(takes(&g, &code(&forty)).is_ok());
    assert_eq!(takes(&g, &code(&(forty + "\\n"))), Err(Stop::Masked));
    assert!(takes(&g, &code(&"a".repeat(120))).is_ok());
    assert_eq!(takes(&g, &code(&"a".repeat(121))), Err(Stop::Masked));
}

#[test]
fn a_list_takes_at_most_its_items() {
    let acts = actions();
    let g = Tagged::new(&acts).unwrap();
    let six = r#"{"name":"clarify","arguments":{"question":"q","options":["1","2","3","4","5","6""#;
    assert!(whole(&g, &format!("{six}]}}}}")));
    assert_eq!(takes(&g, &format!("{six},")), Err(Stop::Masked));
}

#[test]
fn jump_forward_takes_the_literals_and_a_word_once_it_is_picked() {
    let acts = actions();
    let g = Tagged::new(&acts).unwrap();
    let mut out = Vec::new();
    g.forced(&g.start(), &mut out);
    assert_eq!(out, OPEN);
    let s = takes(&g, r#"{"name":"set_p"#).unwrap();
    g.forced(&s, &mut out);
    assert_eq!(out, br#"olicy","arguments":{"name":""#);
    let s = takes(&g, r#"{"name":"set_policy","arguments":{"name":"r"#).unwrap();
    g.forced(&s, &mut out);
    assert_eq!(out, br#"ound-robin"}}"#);
    assert!(g.is_done(&g.advance(s, &out).unwrap()));
    let s = takes(&g, r#"{"name":"cpus""#).unwrap();
    g.forced(&s, &mut out);
    assert_eq!(out, br#","arguments":{}}"#);
    let s = takes(&g, r#"{"name":"say","arguments":{"text":""#).unwrap();
    g.forced(&s, &mut out);
    assert!(out.is_empty());
    assert!(g.heals());
}

#[test]
fn need_is_never_more_than_the_bytes_left() {
    let acts = actions();
    let g = Tagged::new(&acts).unwrap();
    let s = takes(&g, r#"{"name":"say","arguments":{"text":"hi"#).unwrap();
    assert_eq!(g.need(&s), 3);
    let s = takes(&g, r#"{"name":"undo","arguments":{"steps":"#).unwrap();
    assert_eq!(g.need(&s), 3);
    for text in [
        r#"{"name":"editor_open","arguments":{"path":"/a","line":12}}"#,
        r#"{"name":"clarify","arguments":{"question":"q","options":["a","bc"]}}"#,
        r#"{"name":"script","arguments":{"code":"a\nb","lang":"rhai"}}"#,
    ] {
        for cut in 0..text.len() {
            let s = takes(&g, &text[..cut]).unwrap();
            assert!(g.need(&s) <= text.len() - cut, "{}", &text[..cut]);
            assert!(g.need(&s) > 0);
        }
    }
}

#[test]
fn check_refuses_what_the_state_cannot_hold() {
    let bad = |args: &'static [Arg<'static>]| Tagged::new(&[Action { name: "a", args }]).is_err();
    assert!(Tagged::new(&[]).is_err());
    assert!(
        Tagged::new(&[Action {
            name: "a b",
            args: &[]
        }])
        .is_err()
    );
    assert!(
        Tagged::new(
            &[Action {
                name: "a",
                args: &[]
            }; 2]
        )
        .is_err()
    );
    assert!(bad(&[Arg {
        name: "n",
        kind: Kind::Int { min: -1, max: 3 }
    }]));
    assert!(bad(&[Arg {
        name: "n",
        kind: Kind::Int { min: 4, max: 3 }
    }]));
    assert!(bad(&[Arg {
        name: "w",
        kind: Kind::Enum(&["a\"b"])
    }]));
    assert!(bad(&[Arg {
        name: "w",
        kind: Kind::Enum(&["a", "a"])
    }]));
    assert!(bad(&[Arg {
        name: "l",
        kind: Kind::List(&Kind::List(&POLICY, 2), 2)
    }]));
    assert!(bad(&[Arg {
        name: "o",
        kind: Kind::Opt(&Kind::Opt(&POLICY))
    }]));
    assert!(bad(&[
        Arg {
            name: "x",
            kind: POLICY
        },
        Arg {
            name: "x",
            kind: POLICY
        }
    ]));
}

const ROOMY: Budget = Budget {
    kv_bytes: 64 << 20,
    other_bytes: 64 << 20,
};

/// Answers under the grammar, drawn at the script temperature.
fn answer(engine: &mut Engine<'_>, g: &Tagged<'_>, user: &str, seed: u64) -> crate::chat::Reply {
    let turns = [
        Turn::new(Role::System, "Answer with one call."),
        Turn::new(Role::User, user),
    ];
    let mut sampler = Sampler::seeded(0.2, seed);
    engine
        .reply_under(&turns, 96, &mut sampler, Some(g), true, |_| Next::Continue)
        .unwrap()
}

#[test]
fn the_engine_answers_alike_warm_or_fresh_on_every_tier() {
    let bytes = fixture::build(&Spec::tiny());
    let acts = actions();
    let g = Tagged::new(&acts).unwrap();
    let requests = [
        "switch to round robin",
        "hello",
        "undo that",
        "switch to round robin",
    ];
    let mut first: Option<Vec<Vec<u8>>> = None;
    for tier in Tier::all() {
        let mut warm = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
        let mut answers = Vec::new();
        for (i, request) in requests.iter().enumerate() {
            let mut fresh = Engine::load(&bytes, tier, 256, ROOMY).unwrap();
            let expected = answer(&mut fresh, &g, request, i as u64);
            let got = answer(&mut warm, &g, request, i as u64);
            let same = |r: &crate::chat::Reply| (r.bytes.clone(), r.tokens, r.forced, r.ending);
            assert_eq!(same(&got), same(&expected), "{request}");
            assert_cache_holds_its_tokens(&bytes, tier, &warm);
            // Whatever it says is a prefix of a call; a closed one parses.
            let state = g.advance(g.start(), &got.bytes).unwrap();
            if g.is_done(&state) {
                let call: serde_json::Value = serde_json::from_slice(&got.bytes).unwrap();
                assert!(call["arguments"].is_object());
            }
            assert!(got.forced > 0 && got.forced <= got.tokens);
            answers.push(got.bytes);
        }
        match &first {
            None => first = Some(answers),
            Some(seen) => assert_eq!(&answers, seen, "{} tier", tier.name()),
        }
    }
}

/// The mask split over several workers allows the same tokens as one worker's,
/// so the answers are the same.
#[test]
fn the_mask_on_many_workers_answers_as_on_one() {
    let bytes = fixture::build(&Spec::tiny());
    let acts = actions();
    let g = Tagged::new(&acts).unwrap();
    let threads = crate::kquant_tests::Threads(3);
    for (i, request) in ["switch to round robin", "hello", "undo that"]
        .iter()
        .enumerate()
    {
        let mut one = Engine::load(&bytes, Tier::best(), 256, ROOMY).unwrap();
        let mut many = Engine::load(&bytes, Tier::best(), 256, ROOMY).unwrap();
        many.set_parallel(&threads);
        let (a, b) = (
            answer(&mut one, &g, request, i as u64),
            answer(&mut many, &g, request, i as u64),
        );
        assert_eq!(
            (a.bytes, a.tokens, a.ending),
            (b.bytes, b.tokens, b.ending),
            "{request}"
        );
    }
}
