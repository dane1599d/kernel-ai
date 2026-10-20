//! Building the summary the model reads, within the prompt budget, and the
//! numbers the proposal card shows first.

use super::{
    BUSY_WAKES_PER_S, INTENT_MAX_CHARS, Label, MAX_PROCESSES, Observation, PROMPT_BUDGET,
    ProcessRow, SLOW_WAIT_US, Summary, VERDICT_MAX_CHARS, estimate_tokens, system_prompt, two_sig,
    unique_names,
};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// A row's fixed part: everything but the quoted name, which leads it.
fn row_rest(row: &ProcessRow, label: &str) -> String {
    format!(
        " {} {} {} {label}\n",
        two_sig(row.cpu_permille, 1),
        two_sig(row.wakes_per_s, 0),
        two_sig(row.mean_burst_us, 3),
    )
}

/// `text` as one line of at most `chars` printable ASCII characters inside
/// quotes: anything else, and any quote, backslash or angle bracket that could
/// end the quotes or look like a chat-template marker, becomes `'` or `?`.
fn quoted(text: &str, chars: usize) -> String {
    let flat: String = text
        .chars()
        .take_while(|&ch| ch != '\n')
        .take(chars)
        .map(|ch| match ch {
            '"' | '\\' => '\'',
            '<' | '>' => '?',
            ' '..='~' => ch,
            _ => '?',
        })
        .collect();
    format!("\"{}\"", flat.trim())
}

fn label_of(row: &ProcessRow, is_target: bool) -> &'static str {
    match (is_target, row.protected, row.label) {
        (true, _, _) => "target",
        (_, true, _) => "protected",
        (_, _, Label::Background) => "background",
        _ => "normal",
    }
}

/// The summary's first lines, with at most `intent_chars` of the founder's words
/// and the `verdicts` newest trial verdicts.
fn head(
    observation: &Observation,
    target_name: &str,
    intent_chars: usize,
    verdicts: usize,
) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "Goal: keep \"{target_name}\" responsive; background work must progress."
    );
    let intent = observation
        .intent
        .as_deref()
        .filter(|i| !i.trim().is_empty());
    if let Some(intent) = intent.filter(|_| intent_chars > 0) {
        let _ = writeln!(text, "Founder: {}", quoted(intent, intent_chars));
    }
    let shown: Vec<String> = observation
        .verdicts
        .iter()
        .take(verdicts)
        .map(|v| quoted(v, VERDICT_MAX_CHARS))
        .collect();
    let verdicts = match shown.is_empty() {
        true => String::from("none"),
        false => shown.join(", "),
    };
    let _ = writeln!(
        text,
        "Now: {}; trials: {verdicts}; {} CPU, {}% busy.\nname cpu% wakes/s burst-ms label",
        observation.current.label(),
        observation.cpus,
        two_sig(observation.busy_permille, 1),
    );
    text
}

/// The rows that may be shown, in order: the target first, then the rest by CPU,
/// at most [`MAX_PROCESSES`]. `None` if the target is not a process.
pub(super) fn candidates(observation: &Observation) -> Option<Vec<usize>> {
    let target = observation.target;
    let rows = &observation.processes;
    if target >= rows.len() {
        return None;
    }
    let mut order: Vec<usize> = (0..rows.len()).filter(|&i| i != target).collect();
    order.sort_by_key(|&i| (u64::MAX - rows[i].cpu_permille, i));
    order.insert(0, target);
    order.truncate(MAX_PROCESSES);
    Some(order)
}

/// The summary of `observation`, within [`PROMPT_BUDGET`] by the token estimate;
/// `None` if its target is not one of its processes.
pub fn summary(observation: &Observation) -> Option<Summary> {
    summary_with(observation, &estimate_tokens, PROMPT_BUDGET)
}

/// The summary with rows added while `count`, a token count for a piece of the
/// prompt, keeps the system prompt and the summary within `budget`. Lines are
/// counted one by one, which adds up exactly, since Qwen3's pre-tokenizer never
/// joins text across a newline. With the model's own tokenizer as `count` the
/// prompt fits for any names; with the estimate it fits for ordinary ones.
pub fn summary_with(
    observation: &Observation,
    count: &dyn Fn(&str) -> usize,
    budget: usize,
) -> Option<Summary> {
    let order = candidates(observation)?;
    let rows = &observation.processes;
    let names = unique_names(rows);
    let target = observation.target;
    let target_name = names[target].clone();
    let wait = |us| two_sig(us, 3);
    let mut flags = String::from("Flags:");
    let _ = match rows[target].wait_p99_us >= SLOW_WAIT_US {
        true => write!(
            flags,
            " \"{target_name}\" waits {} ms at p99",
            wait(rows[target].wait_p99_us)
        ),
        false => write!(flags, " \"{target_name}\" waits under 2 ms; keep is fine"),
    };
    let _ = write!(
        flags,
        " (p50 {}, max {}).",
        wait(rows[target].wait_p50_us),
        wait(rows[target].wait_max_us)
    );
    let lines = |text: &str| text.lines().map(count).sum::<usize>();
    let row = |i: usize| {
        let label = label_of(&rows[i], i == target);
        format!("\"{}\"{}", names[i], row_rest(&rows[i], label))
    };
    // Reserved up front: the cards, the flags, the omitted-processes note. The
    // founder's words and old verdicts shrink, if they must, so the target's own
    // row always fits.
    let reserve = count("(16 more processes not shown; they cannot be labelled.)")
        + lines(system_prompt())
        + count(&flags)
        + count(super::NO_THINK);
    let mut text = String::new();
    for (intent_chars, verdicts) in [(INTENT_MAX_CHARS, 3), (INTENT_MAX_CHARS / 2, 1), (0, 0)] {
        text = head(observation, &target_name, intent_chars, verdicts);
        if reserve + lines(&text) + count(row(target).trim_end()) <= budget {
            break;
        }
    }
    let mut used = reserve + lines(&text);
    let (mut shown, mut labelable) = (Vec::new(), Vec::new());
    for &i in &order {
        let line = row(i);
        let cost = count(line.trim_end());
        if i != target && used + cost > budget {
            continue;
        }
        used += cost;
        text.push_str(&line);
        shown.push((i, names[i].clone()));
        if i != target && !rows[i].protected {
            labelable.push(names[i].clone());
        }
    }
    let omitted = rows.len() - shown.len();
    if omitted > 0 {
        let _ = writeln!(
            text,
            "({omitted} more processes not shown; they cannot be labelled.)"
        );
    }
    text.push_str(&flags);
    Some(Summary {
        text,
        target,
        shown,
        labelable,
        target_name,
        omitted,
    })
}

/// The numbers the proposal card shows first, chosen by code.
pub fn measured(observation: &Observation, summary: &Summary) -> String {
    let mut parts = Vec::new();
    if let Some(row) = observation.processes.get(summary.target) {
        parts.push(format!(
            "\"{}\" waits {} ms at p99",
            summary.target_name,
            two_sig(row.wait_p99_us, 3)
        ));
    }
    let mut wakers: Vec<(u64, &str)> = summary
        .shown
        .iter()
        .filter(|(index, _)| *index != summary.target)
        .filter_map(|(index, name)| {
            let row = observation.processes.get(*index)?;
            (row.wakes_per_s >= BUSY_WAKES_PER_S).then_some((row.wakes_per_s, name.as_str()))
        })
        .collect();
    wakers.sort_by_key(|&(wakes, _)| u64::MAX - wakes);
    let busiest: Vec<String> = wakers
        .iter()
        .take(3)
        .map(|(wakes, name)| format!("\"{name}\" {}/s", two_sig(*wakes, 0)))
        .collect();
    if !busiest.is_empty() {
        parts.push(format!("busiest wakers {}", busiest.join(", ")));
    }
    parts.push(format!(
        "CPU {}% busy",
        two_sig(observation.busy_permille, 1)
    ));
    format!("{}.", parts.join("; "))
}
