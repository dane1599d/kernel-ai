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

/// What llm compares to tell whether a request's apps are those of the
/// grammar in use: each of [`entries`]' (app, its answer's first line),
/// sorted by app. The shell lists running apps first, so an app starting or
/// stopping reorders the same set; sorted, that is no new key,
/// so no grammar is built (and leaked) again and the apps turn, with the act
/// slot's cached prefix, stays as it was (bug hunt #21).
pub fn key(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut key: Vec<(String, String)> = entries
        .iter()
        .map(|(app, answer)| {
            let head = answer.lines().next().unwrap_or("");
            (String::from(*app), String::from(head))
        })
        .collect();
    key.sort();
    key
}

/// The group in `groups` holding the row `name`.
pub fn group_of<'a>(groups: &'a [AppGroup], name: &str) -> Option<&'a AppGroup> {
    groups
        .iter()
        .find(|group| group.rows.iter().any(|row| row.name == name))
}
