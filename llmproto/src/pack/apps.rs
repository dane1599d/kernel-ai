//! Installed apps' groups (P4 task 15, ADR 0010 §3 and §6): the rows each
//! app's `kai-actions 1` declares, as the shell loaded them, shown to the model
//! in a turn of their own after MODE so the static pack and its `pack_id` stay
//! as they are. The turn is on the index level: one line of row names per app,
//! `notes: notes_add notes_list`. A call naming an app row stops after the name
//! and is decoded once more with that app's full lines ([`group_text`]) after
//! the request, as the pack's own index groups are.
//!
//! The OS renders every line from the checked rows; an app supplies no prompt
//! text but its rows' names and docs (ADR 0010 §2).

use super::{dsl1, estimate_tokens};
use agentabi::Action;
use alloc::string::String;
use alloc::vec::Vec;

/// The apps turn's budget, in estimated tokens. Under ADR 0010 §4's 768,
/// which assumed a 3,072-token context: at P1's 2,048, pack and MODE (about
/// 942 real) and the rest of the turn (512) leave history 512 of 594, and a
/// full apps turn still leaves it about 330, over `HISTORY_FLOOR`.
pub const APPS_BUDGET: usize = 256;
/// The apps turn's first line.
pub const APPS_HEADING: &str = "## apps (call one and its arguments are shown next)";

/// One installed app's rows, in the order its `kai-actions 1` declares them.
#[derive(Clone, Debug)]
pub struct AppGroup {
    pub app: String,
    pub rows: Vec<&'static Action>,
}

/// `notes: notes_add notes_list`.
pub fn line(group: &AppGroup) -> String {
    let mut line = alloc::format!("{}:", group.app);
    for row in &group.rows {
        line.push(' ');
        line.push_str(row.name);
    }
    line
}

/// `text`'s tokens with no tokenizer at hand: the pack's bytes estimate, or
/// one that counts every digit and short word as Qwen3's pre-tokenizer splits
/// them (`crate::tune::estimate_tokens`), whichever is more. An app's row names
/// are its own choice: `ab_1c2d3e4f` is a token a character, three times the
/// bytes estimate. Only the real tokenizer bounds every name ([`turn_counted`]).
pub fn estimate(text: &str) -> usize {
    estimate_tokens(text).max(crate::tune::estimate_tokens(text))
}

/// The apps turn for `groups`, and the apps left out of it: those whose line
/// would take it past [`APPS_BUDGET`] by [`estimate`], and those with no rows.
/// "" for none.
pub fn turn(groups: &[AppGroup]) -> (String, Vec<String>) {
    turn_counted(groups, estimate)
}

/// [`turn`], with `count` giving each text's tokens: llm counts with the
/// model's own tokenizer, so no row name passes the budget however it splits.
pub fn turn_counted(groups: &[AppGroup], count: impl Fn(&str) -> usize) -> (String, Vec<String>) {
    let mut text = String::from(APPS_HEADING);
    let mut shown = 0;
    let mut left_out = Vec::new();
    for group in groups {
        let line = line(group);
        if group.rows.is_empty() || count(&text) + count(&line) > APPS_BUDGET {
            left_out.push(group.app.clone());
            continue;
        }
        text.push('\n');
        text.push_str(&line);
        shown += 1;
    }
    match shown {
        0 => (String::new(), left_out),
        _ => (text, left_out),
    }
}

/// The flat apps turn's first line (spike S10-2's flat arm).
pub const FLAT_HEADING: &str = "## apps";

/// Spike S10-2's flat arm: every app's full lines ([`group_text`]) in the apps
/// turn, with no budget, so a call naming an app row is written in one round.
/// Apps with no rows are left out; "" for none. What ships is [`turn`].
pub fn flat_turn(groups: &[AppGroup]) -> (String, Vec<String>) {
    let mut text = String::from(FLAT_HEADING);
    let mut left_out = Vec::new();
    for group in groups {
        if group.rows.is_empty() {
            left_out.push(group.app.clone());
            continue;
        }
        text.push('\n');
        text.push_str(group_text(group).trim_end());
    }
    match left_out.len() == groups.len() {
        true => (String::new(), left_out),
        false => (text, left_out),
    }
}

/// The most words of a row's doc [`described_turn`] tries.
pub const DESCRIBED_MAX_WORDS: usize = 12;

/// Words [`short_doc`] leaves out: articles anywhere, and these only as the
/// last word after a cut.
const ARTICLES: [&str; 3] = ["a", "an", "the"];
const TRAILING: [&str; 13] = [
    "on", "to", "for", "of", "with", "in", "off", "by", "at", "that", "are", "is", "this",
];

/// A row's short description for spike S10-2's described arm: its doc up to
/// the first ", " or ".", without articles, at most `words` words, with no
/// preposition left dangling at the end and a lower-case first letter
/// (`The events on a day, like today.` is `events on day`).
pub fn short_doc(doc: &str, words: usize) -> String {
    let end = [doc.find(", "), doc.find('.')]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(doc.len());
    let mut kept: Vec<&str> = doc[..end]
        .split_whitespace()
        .filter(|word| !ARTICLES.contains(&word.to_lowercase().as_str()))
        .take(words)
        .collect();
    while kept.len() > 1 && kept.last().is_some_and(|w| TRAILING.contains(w)) {
        kept.pop();
    }
    let text = kept.join(" ");
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// [`line`] with each row's [`short_doc`] of at most `words` words after its
/// name: `music: music_play (play a song) music_skip (skip to the next song)`.
pub fn described_line(group: &AppGroup, words: usize) -> String {
    let mut line = alloc::format!("{}:", group.app);
    for row in &group.rows {
        line.push(' ');
        line.push_str(row.name);
        let doc = short_doc(row.doc, words);
        if !doc.is_empty() {
            line.push_str(" (");
            line.push_str(&doc);
            line.push(')');
        }
    }
    line
}

/// Spike S10-2's described arm's apps turn (see [`described_turn`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Described {
    /// The turn, "" for none.
    pub text: String,
    /// The words each description was cut to.
    pub words: usize,
    /// Apps left out, as [`turn_counted`] leaves them out.
    pub left_out: Vec<String>,
    /// Apps shown with their plain [`line`]: no room for a description.
    pub plain: Vec<String>,
}

/// Spike S10-2's described arm: [`turn_counted`] with each app's
/// [`described_line`], under the same heading and [`APPS_BUDGET`] by the same
/// `count`. Every description is cut to the most words, from
/// [`DESCRIBED_MAX_WORDS`] down to 1, at which every app that [`turn_counted`]
/// would show fits with its descriptions. If none does, it is cut to 1 word,
/// and an app whose described line would pass the budget gets its plain
/// [`line`] instead, and is left out only when that does not fit either. What
/// ships is [`turn`].
pub fn described_turn(groups: &[AppGroup], count: impl Fn(&str) -> usize) -> Described {
    let shipped_left_out = turn_counted(groups, &count).1;
    let mut last = None;
    for words in (1..=DESCRIBED_MAX_WORDS).rev() {
        let turn = described_at(groups, &count, words);
        if turn.plain.is_empty() && turn.left_out == shipped_left_out {
            return turn;
        }
        last = Some(turn);
    }
    last.unwrap_or_else(|| described_at(groups, &count, 1))
}

/// [`described_turn`] with every description cut to `words` words.
fn described_at(groups: &[AppGroup], count: &impl Fn(&str) -> usize, words: usize) -> Described {
    let mut text = String::from(APPS_HEADING);
    let mut shown = 0;
    let (mut left_out, mut plain) = (Vec::new(), Vec::new());
    for group in groups {
        let fits = |candidate: &str| count(&text) + count(candidate) <= APPS_BUDGET;
        let described = described_line(group, words);
        let plain_line = line(group);
        let chosen = if group.rows.is_empty() {
            None
        } else if fits(&described) {
            Some(described)
        } else if fits(&plain_line) {
            plain.push(group.app.clone());
            Some(plain_line)
        } else {
            None
        };
        let Some(chosen) = chosen else {
            left_out.push(group.app.clone());
            continue;
        };
        text.push('\n');
        text.push_str(&chosen);
        shown += 1;
    }
    if shown == 0 {
        text = String::new();
    }
    Described {
        text,
        words,
        left_out,
        plain,
    }
}

/// App `group`'s full lines, as a system turn after the request: what a call
/// naming one of its rows is decoded with the second time. Each row has a line
/// of its own; the OS's family lines (`service_stop|start`) are for its rows.
pub fn group_text(group: &AppGroup) -> String {
    let mut text = alloc::format!("## {}\n", group.app);
    for row in &group.rows {
        text.push_str(&dsl1::line(row));
        text.push('\n');
    }
    text
}

/// The `apps` text of an `act` request: `app <name> <len>`, a newline and the
/// app's `kai-actions 1` answer, for each of `answers` (app, answer) whose
/// whole entry still fits in `max` bytes; and the apps left out.
pub fn wire(answers: &[(&str, &str)], max: usize) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut left_out = Vec::new();
    for (app, answer) in answers {
        let entry = alloc::format!("app {app} {}\n{answer}", answer.len());
        match text.len() + entry.len() <= max {
            true => text.push_str(&entry),
            false => left_out.push(String::from(*app)),
        }
    }
    (text, left_out)
}

/// The (app, answer) entries of [`wire`]'s text, or `None` if it is not that.
pub fn entries(mut text: &str) -> Option<Vec<(&str, &str)>> {
    let mut entries = Vec::new();
    while !text.is_empty() {
        let (head, rest) = text.split_once('\n')?;
        let (app, len) = head.strip_prefix("app ")?.split_once(' ')?;
        let len: usize = len.parse().ok()?;
        let valid = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if app.is_empty() || !app.chars().all(valid) {
            return None;
        }
        entries.push((app, rest.get(..len)?));
        text = &rest[len..];
    }
    Some(entries)
}

/// The group in `groups` holding the row `name`.
pub fn group_of<'a>(groups: &'a [AppGroup], name: &str) -> Option<&'a AppGroup> {
    groups
        .iter()
        .find(|group| group.rows.iter().any(|row| row.name == name))
}
