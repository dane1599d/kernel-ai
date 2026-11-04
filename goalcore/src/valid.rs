//! What a goal's words may be: the checks a request and a record read back from
//! disk are held to, and the scope's matching.

use crate::goal::Spec;
use crate::{
    MAX_CHECKINS, MAX_FOLDERS, MAX_ROWS, MAX_STEPS, MAX_TEXT, MAX_TOKENS, MAX_WALL_MS, ROW_CHARS,
};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Printable ASCII only: no tab or newline to break a record's lines, and no
/// control character or look-alike to hide in a goal's words.
pub(crate) fn printable(text: &str) -> bool {
    text.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// Words for a goal's text, a note or a why: printable, not empty, not padded.
pub(crate) fn words_ok(text: &str, max: usize) -> bool {
    !text.is_empty()
        && text.len() <= max
        && printable(text)
        && !text.starts_with(' ')
        && !text.ends_with(' ')
}

/// Words that may also be empty (a note not yet written).
pub(crate) fn note_ok(text: &str) -> bool {
    text.is_empty() || words_ok(text, MAX_TEXT)
}

fn ident_ok(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// An exact row name (`fs_list`): `[a-z][a-z0-9_]*`.
pub(crate) fn row_name_ok(name: &str) -> bool {
    name.len() <= ROW_CHARS && ident_ok(name)
}

/// A scope entry: an exact row, or a group as the manifest writes it (`notes_*`).
pub(crate) fn scope_row_ok(row: &str) -> bool {
    match row.strip_suffix("_*") {
        Some(group) => group.len() < ROW_CHARS && ident_ok(group),
        None => row_name_ok(row),
    }
}

/// A path under the home folder: relative, with no empty, `.` or `..` part, so it
/// cannot leave the folder it is checked against.
pub(crate) fn path_ok(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 255
        && printable(path)
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Whether `row` is one the scope names.
pub(crate) fn row_in(rows: &[String], row: &str) -> bool {
    row_name_ok(row)
        && rows.iter().any(|entry| match entry.strip_suffix('*') {
            Some(prefix) => row.starts_with(prefix),
            None => entry == row,
        })
}

/// Whether `path` is one of the folders or inside one. A path that is not a plain
/// relative one is never inside any.
pub(crate) fn path_in(folders: &[String], path: &str) -> bool {
    path_ok(path)
        && folders.iter().any(|folder| {
            path == folder
                || path
                    .strip_prefix(folder.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
}

fn all_distinct(items: &[String]) -> bool {
    items
        .iter()
        .enumerate()
        .all(|(at, item)| !items[..at].contains(item))
}

/// Why `spec` cannot be a goal made at `now`.
pub(crate) fn spec(spec: &Spec, now: u64) -> Result<(), String> {
    if !words_ok(&spec.text, MAX_TEXT) {
        return Err(format!(
            "a goal's words are 1 to {MAX_TEXT} printable characters"
        ));
    }
    let rows = &spec.scope.rows;
    if rows.is_empty() || rows.len() > MAX_ROWS {
        return Err(format!("a goal names 1 to {MAX_ROWS} rows it may use"));
    }
    if let Some(row) = rows.iter().find(|row| !scope_row_ok(row)) {
        return Err(format!("{row} is not a row or a group of rows"));
    }
    if !all_distinct(rows) {
        return Err(String::from("a row is named twice"));
    }
    let folders = &spec.scope.folders;
    if folders.len() > MAX_FOLDERS {
        return Err(format!("a goal names at most {MAX_FOLDERS} folders"));
    }
    if let Some(folder) = folders.iter().find(|folder| !path_ok(folder)) {
        return Err(format!("{folder} is not a folder under your home folder"));
    }
    if !all_distinct(folders) {
        return Err(String::from("a folder is named twice"));
    }
    let budget = &spec.budget;
    if budget.steps == 0 || budget.steps > MAX_STEPS {
        return Err(format!("a goal runs 1 to {MAX_STEPS} steps"));
    }
    if budget.tokens == 0 || budget.tokens > MAX_TOKENS {
        return Err(format!("a goal spends 1 to {MAX_TOKENS} tokens"));
    }
    if budget.wall_ms == 0 || budget.wall_ms > MAX_WALL_MS {
        return Err(String::from("a goal takes at most 30 days"));
    }
    if spec.by_ms.is_some_and(|by| by <= now) {
        return Err(String::from("the deadline is not in the future"));
    }
    let times = &spec.checkins;
    if times.len() > MAX_CHECKINS {
        return Err(format!("a goal has at most {MAX_CHECKINS} check-ins"));
    }
    let ascending = times.windows(2).all(|pair| pair[0] < pair[1]);
    if !ascending || times.first().is_some_and(|&first| first <= now) {
        return Err(String::from(
            "check-ins are in the future, each later than the one before",
        ));
    }
    if let (Some(by), Some(&last)) = (spec.by_ms, times.last())
        && last > by
    {
        return Err(String::from("a check-in is after the deadline"));
    }
    Ok(())
}

fn join(items: &[String]) -> String {
    let mut out = String::new();
    for (at, item) in items.iter().enumerate() {
        if at > 0 {
            out.push_str(", ");
        }
        out.push_str(item);
    }
    out
}

/// What the ask shows before the person says yes.
pub(crate) fn approval(spec: &Spec) -> String {
    let mut out = format!("Goal: {}\n", spec.text);
    match spec.by_ms {
        Some(by) => out.push_str(&format!("By: {by} ms\n")),
        None => out.push_str("By: no deadline\n"),
    }
    out.push_str(&format!("Rows: {}\n", join(&spec.scope.rows)));
    match spec.scope.folders.is_empty() {
        true => out.push_str("Folders: none\n"),
        false => out.push_str(&format!("Folders: {}\n", join(&spec.scope.folders))),
    }
    out.push_str(&format!(
        "Budget: {} steps, {} tokens, {} s\n",
        spec.budget.steps,
        spec.budget.tokens,
        spec.budget.wall_ms / 1000
    ));
    let times: Vec<String> = spec.checkins.iter().map(|t| format!("{t} ms")).collect();
    match times.is_empty() {
        true => out.push_str("Check-ins: none set\n"),
        false => out.push_str(&format!("Check-ins: {}\n", join(&times))),
    }
    match spec.away_ok {
        true => out.push_str("Steps that run at once keep going while you are away\n"),
        false => out.push_str("Waits for you when you are away\n"),
    }
    out
}

/// Makes `text` fit a record: printable ASCII (anything else becomes `?`), no
/// padding, and at most [`MAX_TEXT`] bytes. For words that come from a model or a
/// failed step, which a record must keep whatever they hold.
pub(crate) fn clean(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .collect();
    out.truncate(MAX_TEXT);
    String::from(out.trim())
}
