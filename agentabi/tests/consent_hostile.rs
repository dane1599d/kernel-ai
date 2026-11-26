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

fn trash_thesis() -> [Call<'static>; 1] {
    [Call {
        name: "fs_trash",
        args: &[(
            "path",
            Value {
                text: "/home/user/thesis.txt",
                label: Label::Literal,
            },
        )],
        control: Label::Literal,
    }]
}

/// bug-hunt-6 H6-1: the negations a grant already hears turn a change's cue
/// off too: any `n't` or auxiliary `nt`, "cannot", "stop" before an -ing word,
/// and a clause of only negation before the cue's ("never, ever ...").
#[test]
fn every_negation_a_grant_hears_turns_a_change_off() {
    for line in [
        "I didn't ask for round robin",
        "you shouldn't switch to round robin",
        "you can't switch to round robin",
        "you cannot switch to round robin",
        "stop switching to round robin",
        "quit switching to round robin",
        "never, ever switch to round robin",
        "don't, under any circumstances, switch to round robin",
        "didnt switch to round robin",
        "Never, ever. Switch to round robin? No",
    ] {
        let decision = decide(line, &switch(), &ROWS)[0];
        assert_ne!(decision.tier, Tier::Run, "{line:?} ran the switch");
    }
    for line in [
        "you shouldn't trash thesis.txt",
        "Never, ever trash thesis.txt",
        "Don't, under any circumstances, trash thesis.txt",
        "keep thesis.txt, stop trashing it",
    ] {
        let decision = decide(line, &trash_thesis(), &ROWS)[0];
        assert_ne!(decision.tier, Tier::Run, "{line:?} trashed thesis.txt");
    }
    // The same changes asked for plainly still run; a bare "no," still ends at
    // its comma, and a negation still ends at its clause.
    for line in [
        "switch to round robin",
        "no, switch to round robin",
        "don't wait, switch to round robin",
        "I can't find it, switch to round robin",
    ] {
        let decision = decide(line, &switch(), &ROWS)[0];
        assert_eq!(decision.tier, Tier::Run, "{line:?}: {decision:?}");
    }
    // "stop" before the cue's own -ing word (the #1436 review).
    let uninstall = [Call {
        name: "pkg_remove",
        args: &[(
            "name",
            Value {
                text: "calc",
                label: Label::Literal,
            },
        )],
        control: Label::Literal,
    }];
    let decision = decide("keep calc, stop uninstalling it", &uninstall, &ROWS)[0];
    assert_ne!(decision.tier, Tier::Run, "removed calc: {decision:?}");
    let decision = decide("trash thesis.txt", &trash_thesis(), &ROWS)[0];
    assert_eq!(decision.tier, Tier::Run, "{decision:?}");
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

/// One call of `name` with `arg` literal as `text`.
fn one(name: &'static str, arg: &'static str, text: &'static str) -> [Call<'static>; 1] {
    let args: &'static [(&'static str, Value<'static>)] = Box::leak(Box::new([(
        arg,
        Value {
            text,
            label: Label::Literal,
        },
    )]));
    [Call {
        name,
        args,
        control: Label::Literal,
    }]
}

/// The #1436 review, item 1: "stop" or "quit" negates only an action's -ing
/// word in its own clause, and is no negation where it is the row's own verb.
#[test]
fn stop_negates_only_an_action_in_its_clause() {
    // The row's own verb: asked for, so it asks once as a change with no undo,
    // not as one the model added.
    let decision = decide(
        "stop running the miner",
        &one("service_stop", "name", "miner"),
        &ROWS,
    )[0];
    assert_eq!(decision.why, Why::NoUndo, "{decision:?}");
    // "running" in the next sentence is no verb of the stop (#1467 review).
    let decision = decide(
        "Stop! Running the miner is fine, leave it.",
        &one("service_stop", "name", "miner"),
        &ROWS,
    )[0];
    assert_eq!(decision.why, Why::Added, "{decision:?}");
    // The #1460 review: only "running" is the row's own verb. Any other -ing
    // word after it negates, so the call is one the model added and no
    // standing rule ("Always allow process_stop") runs it without a card.
    for (line, call) in [
        (
            "stop shutting down build-3",
            one("process_stop", "name", "build-3"),
        ),
        (
            "stop terminating the miner",
            one("service_stop", "name", "miner"),
        ),
        (
            "stop ending the miner",
            one("service_stop", "name", "miner"),
        ),
        (
            "stop updating the miner",
            one("service_stop", "name", "miner"),
        ),
        ("stop tracking", one("service_stop", "name", "tracking")),
    ] {
        let decision = decide(line, &call, &ROWS)[0];
        assert_eq!(decision.why, Why::Added, "{line:?}: {decision:?}");
    }
    // "Stop." on its own stops the -ing word that opens the next sentence,
    // and a "-thing" word stops what follows it.
    for line in [
        "Stop. Switching to round robin is a mistake",
        "stop anything from switching to round robin",
        "stop everything that switches to round robin",
    ] {
        let decision = decide(line, &switch(), &ROWS)[0];
        assert_ne!(decision.tier, Tier::Run, "{line:?}: {decision:?}");
    }
    // Not an action's -ing word, or past the clause's end: nothing negated.
    let decision = decide(
        "stop everything and switch to round robin",
        &switch(),
        &ROWS,
    )[0];
    assert_eq!(decision.tier, Tier::Run, "{decision:?}");
    let focus = one("ui_focus", "window", "terminal");
    let decision = decide("Stop. Bring up the terminal", &focus, &ROWS)[0];
    assert_eq!(decision.tier, Tier::Run, "{decision:?}");
    // The safe side: "stop" before an action still negates it, for every
    // other row and for the row's own -ing word.
    for (line, call) in [
        (
            "stop running the miner",
            one("service_start", "name", "miner"),
        ),
        (
            "stop killing build-3",
            one("process_stop", "name", "build-3"),
        ),
        (
            "please stop stopping the netd service",
            one("service_stop", "name", "netd"),
        ),
        ("please quit trashing thesis.txt", trash_thesis()),
        ("stop using round robin", switch()),
        ("please stop switching to round robin", switch()),
    ] {
        let decision = decide(line, &call, &ROWS)[0];
        assert_eq!(decision.why, Why::Added, "{line:?}: {decision:?}");
    }
    // The #1460 review: any -ing word but a noun negates, whether or not a
    // row's cue starts it.
    let theme: [Call<'static>; 1] = [Call {
        name: "appearance_set",
        args: &[
            (
                "setting",
                Value {
                    text: "theme",
                    label: Label::Literal,
                },
            ),
            (
                "value",
                Value {
                    text: "dark",
                    label: Label::Literal,
                },
            ),
        ],
        control: Label::Literal,
    }];
    assert_eq!(
        decide("make the theme dark", &theme, &ROWS)[0].tier,
        Tier::Run
    );
    for (line, call) in [
        ("Stop changing the scheduler to round robin", switch()),
        ("stop making the theme dark", theme),
        ("quit putting the scheduler on round robin", switch()),
    ] {
        let decision = decide(line, &call, &ROWS)[0];
        assert_ne!(decision.tier, Tier::Run, "{line:?}: {decision:?}");
    }
    // An app's row whose cue is "stop" and that has an undo would run: its
    // "stop" before an -ing word stays a negation.
    let start = *find(&ROWS, "service_start").expect("service_start");
    let timer_stop = agentabi::Action {
        name: "timer_stop",
        cues: &["stop"],
        owner: agentabi::Owner::App("timer"),
        ..start
    };
    assert_eq!(derive(&timer_stop), Tier::Run);
    let rows = [timer_stop];
    let tea = one("timer_stop", "name", "tea");
    assert_eq!(decide("stop the tea timer", &tea, &rows)[0].tier, Tier::Run);
    for line in [
        "stop restarting the tea timer",
        "please stop running the tea timer",
    ] {
        let decision = decide(line, &tea, &rows)[0];
        assert_ne!(decision.tier, Tier::Run, "{line:?}: {decision:?}");
    }
}

/// The #1436 review, item 2: a name only a negated clause holds is taken by
/// a pointer when that clause only says the person has no more use for it;
/// a name holding a negation word is still a name.
#[test]
fn a_pointer_takes_the_name_the_person_no_longer_needs() {
    for (line, call) in [
        (
            "I don't need calc anymore, remove it",
            one("pkg_remove", "name", "calc"),
        ),
        (
            "we no longer use calc, so please uninstall it",
            one("pkg_remove", "name", "calc"),
        ),
        (
            "trash not-mine.txt",
            one("fs_trash", "path", "/home/user/not-mine.txt"),
        ),
        ("remove no-ads", one("pkg_remove", "name", "no-ads")),
    ] {
        let decision = decide(line, &call, &ROWS)[0];
        assert_eq!(decision.tier, Tier::Run, "{line:?}: {decision:?}");
    }
    // Anything else in or around the negated clause, and it still asks.
    for line in [
        "never touch calc, remove it",
        "I don't want to lose calc, remove it",
        "I don't need calc or notes anymore, remove it",
        "I don't need calc anymore, but don't remove it",
        "I don't need calc anymore? remove it",
        "I don't need calc anymore. Remove it.",
        "you don't need calc, remove it",
        "I don't need calc, remove it and notes",
        "don't remove calc, remove it",
    ] {
        let decision = decide(line, &one("pkg_remove", "name", "calc"), &ROWS)[0];
        assert_ne!(decision.tier, Tier::Run, "{line:?}: {decision:?}");
    }
    let decision = decide(
        "trash not-mine.txt",
        &one("fs_trash", "path", "/home/user/mine.txt"),
        &ROWS,
    )[0];
    assert_ne!(decision.tier, Tier::Run, "{decision:?}");
}

/// The #1436 review, items 3 and 4: a clause whose only negating words are
/// bare answers carries nothing, and no clause carries past its sentence.
#[test]
fn carry_stops_at_an_answer_and_at_a_sentence_end() {
    for line in [
        "ok no, switch to round robin",
        "no really, switch to round robin",
        "so no, switch to round robin",
        "Not again. Switch to round robin.",
        "Neither. Switch to round robin",
    ] {
        let decision = decide(line, &switch(), &ROWS)[0];
        assert_eq!(decision.tier, Tier::Run, "{line:?}: {decision:?}");
    }
    let ideas = one("fs_trash", "path", "/home/user/ideas");
    let decision = decide("neither, just remove ideas please", &ideas, &ROWS)[0];
    assert_eq!(decision.tier, Tier::Run, "{decision:?}");
    // A clause with any other negating word still carries within its sentence.
    for line in [
        "not again, switch to round robin",
        "ok never, switch to round robin",
        "no, never, switch to round robin",
        "never, please, switch to round robin",
        // The #1460 review: only bare answers and their lead-ins make one.
        "under no circumstances, ever, switch to round robin",
        "Under no circumstances, switch to round robin",
        // A run of dots trails off, and a sentence of only a negation is said
        // of the next one.
        "Never... switch to round robin",
        "Don't... switch to round robin",
        "Not. Switch to round robin.",
        "Don't. Switch to round robin",
    ] {
        let decision = decide(line, &switch(), &ROWS)[0];
        assert_ne!(decision.tier, Tier::Run, "{line:?}: {decision:?}");
    }
    let decision = decide(
        "Under no circumstances, delete thesis.txt",
        &trash_thesis(),
        &ROWS,
    )[0];
    assert_ne!(decision.tier, Tier::Run, "{decision:?}");
    let calc = one("pkg_remove", "name", "calc");
    for line in [
        "Don't... remove calc",
        "Do. Not. Remove. Calc.",
        // The #1460 review: a sentence of negation and its filler carries too.
        "Please don't. Remove calc.",
        "Absolutely not. Remove calc.",
    ] {
        let decision = decide(line, &calc, &ROWS)[0];
        assert_ne!(decision.tier, Tier::Run, "{line:?}: {decision:?}");
    }
    // "No." and "Not again." still answer, and say nothing of what follows.
    for line in [
        "No. Switch to round robin",
        "Not again. Switch to round robin.",
    ] {
        let decision = decide(line, &switch(), &ROWS)[0];
        assert_eq!(decision.tier, Tier::Run, "{line:?}: {decision:?}");
    }
}

/// The #1436 review, item 5: "refrain" and "forbid" negate on the change
/// path, as a grant already hears them.
#[test]
fn refrain_and_forbid_turn_a_change_off() {
    for line in [
        "refrain from switching to round robin",
        "please refrain from round robin",
        "I forbid you to switch to round robin",
        "you are forbidden from using round robin",
    ] {
        let decision = decide(line, &switch(), &ROWS)[0];
        assert_ne!(decision.tier, Tier::Run, "{line:?}: {decision:?}");
    }
    let decision = decide(
        "you are forbidden to trash thesis.txt",
        &trash_thesis(),
        &ROWS,
    )[0];
    assert_ne!(decision.tier, Tier::Run, "{decision:?}");
}
