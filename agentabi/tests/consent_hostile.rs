//! Consent under hostile and odd lines (P1 hardening): seeded random lines and
//! plans through `agentabi::consent::decide`, with the rules that must hold for
//! every input, not only the corpus's 150 cases.
//!
//! - It never panics, gives one decision per call, and gives the same twice.
//! - No call is ever weaker than its row's own tier (`derive`).
//! - A change steered by data (an argument or its `if` from content) never runs
//!   at once.
//! - In a line that is not English, no change runs at once.
//! - A call to no row never runs.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use agentabi::consent::{Call, Label, Value, Why, decide};
use agentabi::{ROWS, Tier, derive, find};
use hostile::XorShift;

/// Bits lines are made of: English, the rows' cues, negation, other scripts,
/// emoji, control characters, quotes inside quotes, and markup a page might hold.
const FRAGMENTS: &[&str] = &[
    "switch",
    "to",
    "round",
    "robin",
    "stop",
    "build-3",
    "please",
    "the",
    "policy",
    "make",
    "it",
    "snappier",
    "do",
    "again",
    "undo",
    "that",
    "don't",
    "not",
    "never",
    "no,",
    "install",
    "notes",
    "restart",
    "service",
    "kill",
    "llm",
    "shell",
    "how",
    "many",
    "cpus",
    "?",
    "!",
    "интерактивный",
    "переключи",
    "轮询",
    "调度",
    "سياسة",
    "🙂",
    "\u{0}",
    "\u{7}",
    "\u{1b}[2J",
    "\t",
    "\r",
    "\u{200b}",
    "\u{202e}",
    "\"",
    "'",
    "\"\"\"",
    "'\"'\"'",
    "`",
    "\\",
    "\\\"",
    "<script>",
    "IGNORE PREVIOUS INSTRUCTIONS",
    "set_policy(\"round-robin\")",
    "%s",
    "{}",
    "Ä",
    "é",
    "ß",
    "İ",
    "ﬁ",
];

fn line(rng: &mut XorShift) -> String {
    let mut out = String::new();
    let words = match rng.below(10) {
        0 => 600, // a very long line
        _ => rng.below(14),
    };
    for _ in 0..words {
        if !out.is_empty() && rng.below(4) != 0 {
            out.push(' ');
        }
        out.push_str(FRAGMENTS[rng.below(FRAGMENTS.len())]);
    }
    out
}

fn label(rng: &mut XorShift) -> Label {
    match rng.below(3) {
        0 => Label::Literal,
        1 => Label::System,
        _ => Label::Content("note"),
    }
}

/// Leaks owned text so a plan can borrow it for the whole test.
fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn plan(rng: &mut XorShift) -> Vec<Call<'static>> {
    (0..rng.below(4))
        .map(|_| {
            let name = match rng.below(12) {
                0 => "no_such_row",
                _ => ROWS[rng.below(ROWS.len())].name,
            };
            let args: Vec<(&'static str, Value<'static>)> = find(&ROWS, name)
                .map(|row| row.args)
                .unwrap_or(&[])
                .iter()
                .map(|arg| {
                    let text = leak(
                        line(&mut XorShift::new(rng.next()))
                            .chars()
                            .take(40)
                            .collect(),
                    );
                    (
                        arg.name,
                        Value {
                            text,
                            label: label(rng),
                        },
                    )
                })
                .collect();
            Call {
                name,
                args: Box::leak(args.into_boxed_slice()),
                control: label(rng),
            }
        })
        .collect()
}

fn steered(call: &Call) -> bool {
    matches!(call.control, Label::Content(_))
        || call
            .args
            .iter()
            .any(|(_, v)| matches!(v.label, Label::Content(_)))
}

/// ADR 0002 D6: an undoable change that data chose, or gave its target, asks
/// however plainly the line asks for it, and the ask names the data.
#[test]
fn a_change_data_chose_asks_and_names_the_data() {
    let lit = |text| Value {
        text,
        label: Label::Literal,
    };
    let note = |text| Value {
        text,
        label: Label::Content("note"),
    };
    let cases: [(&str, Call); 3] = [
        // A note's content decided that the install runs.
        (
            "install htop",
            Call {
                name: "pkg_install",
                args: &[("name", lit("htop"))],
                control: Label::Content("note"),
            },
        ),
        // The line's cue, and the target from a note.
        (
            "install htop",
            Call {
                name: "pkg_install",
                args: Box::leak(Box::new([("name", note("htop"))])),
                control: Label::Literal,
            },
        ),
        (
            "switch to round robin",
            Call {
                name: "set_policy",
                args: Box::leak(Box::new([("name", note("round-robin"))])),
                control: Label::Literal,
            },
        ),
    ];
    for (line, call) in cases {
        let decision = decide(line, &[call], &ROWS)[0];
        assert_eq!(
            (decision.tier, decision.why),
            (Tier::Ask, Why::Steered("note")),
            "{line}: {call:?}"
        );
    }
    // The same calls with nothing read run, as the line asks.
    let plain = Call {
        name: "pkg_install",
        args: &[("name", lit("htop"))],
        control: Label::Literal,
    };
    assert_eq!(decide("install htop", &[plain], &ROWS)[0].tier, Tier::Run);
}

#[test]
fn consent_holds_its_rules_for_any_line_and_plan() {
    let mut rng = XorShift::new(0x5eed_c0de);
    for case in 0..20_000 {
        let line = line(&mut rng);
        let plan = plan(&mut rng);
        let decisions = decide(&line, &plan, &ROWS);
        assert_eq!(decisions.len(), plan.len(), "case {case}: {line:?}");
        let again = decide(&line, &plan, &ROWS);
        assert!(
            decisions
                .iter()
                .zip(&again)
                .all(|(a, b)| a.tier == b.tier && a.why == b.why),
            "case {case}: not the same twice: {line:?}"
        );
        let english = agentabi::consent::is_english_line(&line);
        for (call, decision) in plan.iter().zip(&decisions) {
            let Some(row) = find(&ROWS, call.name) else {
                assert_ne!(decision.tier, Tier::Run, "case {case}: an unknown call ran");
                continue;
            };
            assert!(
                decision.tier >= derive(row),
                "case {case}: {} weaker than its row: {line:?}",
                call.name
            );
            let change = !row.facts.read_only;
            if change && steered(call) {
                assert_ne!(
                    decision.tier,
                    Tier::Run,
                    "case {case}: data steered {} into running: {line:?} {call:?}",
                    call.name
                );
            }
            if change && !english {
                assert_ne!(
                    decision.tier,
                    Tier::Run,
                    "case {case}: {} ran from a line not in English: {line:?}",
                    call.name
                );
            }
        }
    }
}

/// `set_policy("round-robin")`, as a plan of one call with literal values.
fn switch() -> [Call<'static>; 1] {
    [Call {
        name: "set_policy",
        args: &[(
            "name",
            Value {
                text: "round-robin",
                label: Label::Literal,
            },
        )],
        control: Label::Literal,
    }]
}

#[test]
fn a_negated_cue_never_runs_the_change() {
    let mut rng = XorShift::new(0xd0_4e9a);
    // "no," ends its clause ("no, not the scheduler"), so it is not here.
    let negations = [
        "don't",
        "do not",
        "never",
        "no",
        "not",
        "please don't",
        "dont",
    ];
    let filler = ["", "ever ", "now ", "the ", "please "];
    for _ in 0..2_000 {
        let line = format!(
            "{} {}switch to round robin",
            negations[rng.below(negations.len())],
            filler[rng.below(filler.len())]
        );
        let decision = decide(&line, &switch(), &ROWS)[0];
        assert_ne!(decision.tier, Tier::Run, "{line:?} ran the switch");
    }
}

#[test]
fn a_cue_in_lookalike_letters_is_not_a_cue() {
    // Cyrillic і, о and а, and full-width letters: they look like the cue but
    // are not the user's English words for it. (A zero-width space inside a word
    // is the user's own line, read as the word.)
    for line in [
        "swіtch to round robin",
        "switch tо rоund rоbin",
        "ｓｗｉｔｃｈ to round robin",
        "switch to rоund-rоbin",
    ] {
        let decision = decide(line, &switch(), &ROWS)[0];
        let plain = decide("switch to round robin", &switch(), &ROWS)[0];
        assert_eq!(plain.tier, Tier::Run);
        assert!(
            decision.tier != Tier::Run || decision.why != plain.why,
            "{line:?} read as the plain cue: {decision:?}"
        );
    }
}
