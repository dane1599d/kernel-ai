//! The proposal card the founder reads before typing yes, and the verdict card
//! code writes after the trial. Both fit one screen: at most
//! [`CARD_MAX_LINES`] lines of at most [`CARD_MAX_COLUMNS`] printable ASCII
//! characters, so no line wraps into something that looks like another line.

use super::{Decision, Summary, two_sig};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use sched::judge::{Reason, Verdict};
use sched::spec::PolicySpec;

pub const CARD_MAX_COLUMNS: usize = 80;
pub const CARD_MAX_LINES: usize = 20;

/// `text` as printable ASCII (anything else becomes `?`), cut into lines of at
/// most [`CARD_MAX_COLUMNS`] at spaces, the later lines indented by two; a word
/// longer than a line is split where it must be.
fn wrap(text: &str) -> Vec<String> {
    let flat: String = text
        .chars()
        .map(|ch| if (' '..='~').contains(&ch) { ch } else { '?' })
        .collect();
    let mut lines: Vec<String> = Vec::new();
    let push = |lines: &mut Vec<String>, line: &str| {
        let indent = if lines.is_empty() { "" } else { "  " };
        lines.push(format!("{indent}{line}"));
    };
    let mut line = String::new();
    for word in flat.split(' ').filter(|w| !w.is_empty()) {
        let mut word = word;
        loop {
            let room = CARD_MAX_COLUMNS - if lines.is_empty() { 0 } else { 2 };
            let needed = match line.is_empty() {
                true => word.len(),
                false => line.len() + 1 + word.len(),
            };
            if needed <= room {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
                break;
            }
            if !line.is_empty() {
                push(&mut lines, &core::mem::take(&mut line));
                continue;
            }
            let (head, tail) = word.split_at(room);
            push(&mut lines, head);
            word = tail;
        }
    }
    if !line.is_empty() {
        push(&mut lines, &line);
    }
    lines
}

fn quoted_list(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("\"{name}\"")).collect();
    quoted.join(", ")
}

/// The proposal card for `decision`, numbered `proposal`.
pub fn card(summary: &Summary, measured: &str, decision: &Decision, proposal: u32) -> Vec<String> {
    let target = &summary.target_name;
    let mut lines = Vec::new();
    lines.extend(wrap(&format!("Tune \"{target}\", proposal {proposal}.")));
    lines.extend(wrap(&format!("Measured: {measured}")));
    let reason = match decision {
        Decision::Keep { reason, .. } | Decision::Try { reason, .. } => reason,
    };
    if !reason.is_empty() {
        lines.extend(wrap(&format!(
            "The model says (its claim, not checked): {reason}"
        )));
    }
    match decision {
        Decision::Keep {
            invalid: Some(why), ..
        } => {
            lines.extend(wrap(&format!("Kept the current policy: {why}.")));
        }
        Decision::Keep { invalid: None, .. } => {
            lines.extend(wrap("Kept the current policy: the model chose to keep it."));
        }
        Decision::Try {
            spec, background, ..
        } => {
            let labels = match background.is_empty() {
                true => String::from("no process labelled background"),
                false => format!("background: {}", quoted_list(background)),
            };
            lines.extend(wrap(&format!("Plan: try {}, {labels}.", spec.label())));
            lines.extend(wrap(
                "Trial: 3 pairs of 3 s windows, the current policy and the new one in turn.",
            ));
            lines.extend(wrap(&format!(
                "Kept only if \"{target}\" p99 latency falls to 0.8x or less, no pair is worse, \
                 and background CPU holds (0.9x in all, 0.8x for each)."
            )));
            lines.extend(wrap(
                "Reverted at once on a wait over 100 ms or an ejection.",
            ));
            lines.extend(wrap(
                "Type yes to start the trial; anything else cancels it.",
            ));
        }
    }
    lines
}

fn ms(ns: u64) -> String {
    two_sig(ns / 1000, 3)
}

fn percent(permille: u64) -> String {
    format!("{}%", two_sig(permille, 1))
}

/// Why the judge reverted, in plain words.
pub fn reason_text(reason: &Reason, names: &[&str], target: &str, verdict: &Verdict) -> String {
    let name = |index: usize| names.get(index).copied().unwrap_or("?");
    match *reason {
        Reason::Guardrail { process, wait_ns } => format!(
            "\"{}\" waited {} ms, over the 100 ms guardrail",
            name(process),
            ms(wait_ns)
        ),
        Reason::Ejected => String::from("the watchdog ejected the policy"),
        Reason::TooFewSamples => format!("too few \"{target}\" events to judge"),
        Reason::NotFaster => format!(
            "\"{target}\" p99 went from {} to {} ms, not down to 0.8x",
            ms(verdict.baseline_p99_ns),
            ms(verdict.candidate_p99_ns)
        ),
        Reason::PairWorse { pair } => {
            format!("\"{target}\" p99 was worse in window pair {}", pair + 1)
        }
        Reason::BackgroundTotal { permille } => format!(
            "background CPU fell to {} of before (it must keep 90%)",
            percent(permille)
        ),
        Reason::NotRun => {
            String::from("the trial could not run: the current policy was ejected first")
        }
        Reason::BackgroundProcess { process, permille } => format!(
            "\"{}\" got {} of its CPU (each must keep 80%)",
            name(process),
            percent(permille)
        ),
    }
}

/// The verdict card, in code's words. `names` are the processes by index.
pub fn verdict_card(
    spec: PolicySpec,
    names: &[&str],
    target: &str,
    verdict: &Verdict,
) -> Vec<String> {
    let mut lines = Vec::new();
    let numbers = format!(
        "\"{target}\" p99 latency {} -> {} ms ({}x); background CPU {} of before",
        ms(verdict.baseline_p99_ns),
        ms(verdict.candidate_p99_ns),
        two_sig(verdict.ratio_permille().min(99_000), 3),
        percent(verdict.background_permille)
    );
    let least = verdict
        .worst_process
        .map(|(index, share)| {
            format!(
                "; least: \"{}\" {}",
                names.get(index).copied().unwrap_or("?"),
                percent(share)
            )
        })
        .unwrap_or_default();
    match &verdict.reason {
        None => lines.extend(wrap(&format!("Kept {}: {numbers}{least}.", spec.label()))),
        Some(reason) => {
            lines.extend(wrap(&format!(
                "Rolled back {}: {}.",
                spec.label(),
                reason_text(reason, names, target, verdict)
            )));
            lines.extend(wrap(&format!("Measured: {numbers}{least}.")));
        }
    }
    lines
}

#[cfg(test)]
mod tests;
