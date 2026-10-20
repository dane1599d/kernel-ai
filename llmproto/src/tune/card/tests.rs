use super::super::tests::mix_a;
use super::super::{Decision, measured, summary};
use super::*;
use alloc::string::String;
use sched::judge::{Reason, Verdict};
use sched::spec::PolicySpec;

#[test]
fn cards_fit_one_screen() {
    let observation = mix_a();
    let summary = summary(&observation).unwrap();
    let measured = measured(&observation, &summary);
    assert!(measured.contains("\"editor\" waits 4.8 ms at p99"));
    assert!(measured.contains("\"indexer\" 410/s"));
    let names: Vec<String> = (0..8).map(|i| format!("{i}abcdefghijklmn{i}")).collect();
    let decisions = [
        Decision::Try {
            reason: format!("{} {}", "w".repeat(100), "word ".repeat(12)),
            spec: PolicySpec::parse("classes", "firm").unwrap(),
            background: names,
        },
        Decision::Keep {
            reason: String::from("Nothing to do."),
            invalid: Some(String::from("the answer was not the JSON object asked for")),
        },
    ];
    for decision in &decisions {
        let lines = card(&summary, &measured, decision, 12);
        assert!(lines.len() <= CARD_MAX_LINES, "{} lines", lines.len());
        assert!(
            lines
                .iter()
                .all(|l| l.len() <= CARD_MAX_COLUMNS
                    && l.bytes().all(|b| (b' '..=b'~').contains(&b)))
        );
    }
    let names = ["editor", "build-1", "build-2"];
    let verdict = Verdict {
        kept: false,
        reason: Some(Reason::BackgroundProcess {
            process: 2,
            permille: 757,
        }),
        baseline_p99_ns: 4_800_000,
        candidate_p99_ns: 450_000,
        background_permille: 1000,
        worst_process: Some((2, 757)),
    };
    let spec = PolicySpec::parse("responsive", "gentle").unwrap();
    let lines = verdict_card(spec, &names, "editor", &verdict);
    assert!(lines[0].starts_with("Rolled back responsive: \"build-2\" got 76% of its CPU"));
    let kept = Verdict {
        kept: true,
        reason: None,
        ..verdict
    };
    let lines = verdict_card(
        PolicySpec::parse("classes", "firm").unwrap(),
        &names,
        "editor",
        &kept,
    );
    assert!(
        lines[0].starts_with("Kept classes/firm: \"editor\" p99 latency 4.8 -> 0.45 ms (0.093x)")
    );
    assert!(lines.iter().all(|l| l.len() <= CARD_MAX_COLUMNS));
}

#[test]
fn a_reason_of_any_length_wraps_inside_the_screen() {
    let observation = mix_a();
    let summary = summary(&observation).unwrap();
    let spec = PolicySpec::parse("classes", "firm").unwrap();
    for len in 1..=160 {
        for lead in ["", "short words then "] {
            let reason = format!("{lead}{}", "w".repeat(len));
            let decision = Decision::Try {
                reason,
                spec,
                background: Vec::new(),
            };
            for line in card(&summary, "x", &decision, 1) {
                assert!(line.len() <= CARD_MAX_COLUMNS, "{len}: {line:?}");
            }
        }
    }
}
