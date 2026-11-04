//! The `actions` line: the rows an agent may call, which the one door enforces
//! before tiers are even considered (ADR 0013 section 6). An entry is a row's
//! name or `<group>_*`; anything else is refused here, as at the door. A row or
//! group of the OS's own (an OS row group is a reserved package name, ADR 0011
//! section 2) must be in the registry; one of a package's (`notes_*`) is checked
//! when the package is installed, since this crate cannot see what is installed.

use crate::{MAX_ACTIONS, Refusal};
use agentabi::{Action, MAX_NAME, Tier};
use alloc::string::String;
use alloc::vec::Vec;

/// One entry of the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// A row's name, such as `fs_list`.
    Row(String),
    /// A group, written `notes_*`: this holds `notes`.
    Group(String),
}

impl Pattern {
    /// How the entry is written.
    pub fn format(&self) -> String {
        match self {
            Self::Row(name) => name.clone(),
            Self::Group(group) => alloc::format!("{group}_*"),
        }
    }

    /// Whether the entry lets the agent call `row`.
    pub fn allows(&self, row: &str) -> bool {
        match self {
            Self::Row(name) => name == row,
            Self::Group(group) => in_group(row, group),
        }
    }
}

/// Whether `row` belongs to `group`: `<group>_<verb>`, or the row that is the
/// group's whole word (`cpus`).
fn in_group(row: &str, group: &str) -> bool {
    row.strip_prefix(group)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('_'))
}

/// A name's characters: `a-z`, `0-9` and `_`, starting with a letter and not
/// ending with `_`. A package's `-` is `_` in its rows' names (ADR 0011).
fn plain(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_lowercase())
        && !word.ends_with('_')
        && word
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The entry's first word: the OS row group it would be in, if it is one.
fn first_word(name: &str) -> &str {
    name.split('_').next().unwrap_or(name)
}

/// The `actions` line's value: `-` for none, or entries separated by a space.
/// `rows` is the OS's registry.
pub fn parse(value: &str, rows: &[Action]) -> Result<Vec<Pattern>, Refusal> {
    if value == "-" {
        return Ok(Vec::new());
    }
    let words: Vec<&str> = value.split(' ').collect();
    if words.len() > MAX_ACTIONS {
        return Err(Refusal::TooManyActions);
    }
    let mut patterns: Vec<Pattern> = Vec::with_capacity(words.len());
    for word in words {
        let pattern = entry(word, rows)?;
        // A row beside its group, or either twice, says one thing twice: the
        // install card would list it twice and the line could drift.
        if patterns.iter().any(|p| overlaps(p, &pattern)) {
            return Err(Refusal::ActionTwice(word.into()));
        }
        patterns.push(pattern);
    }
    Ok(patterns)
}

/// Whether two entries let the agent call a row in common by name: the same entry,
/// or a row inside a group.
fn overlaps(a: &Pattern, b: &Pattern) -> bool {
    match (a, b) {
        (Pattern::Row(x), Pattern::Row(y)) => x == y,
        (Pattern::Group(x), Pattern::Group(y)) => x == y,
        (Pattern::Group(g), Pattern::Row(r)) | (Pattern::Row(r), Pattern::Group(g)) => {
            in_group(r, g)
        }
    }
}

/// One entry, held to the registry where it is an OS row or group.
fn entry(word: &str, rows: &[Action]) -> Result<Pattern, Refusal> {
    let shape = || Refusal::ActionShape(word.into());
    if let Some(group) = word.strip_suffix("_*") {
        if !plain(group) {
            return Err(shape());
        }
        if !agentabi::is_reserved(group) {
            // A package's group: its name's `-` is `_` in its rows (ADR 0011), and the
            // name rule is the package rules' own.
            kaipkg::header::valid_name(&group.replace('_', "-")).map_err(|_| shape())?;
        } else {
            let mut any = false;
            for row in rows.iter().filter(|row| in_group(row.name, group)) {
                any = true;
                if row.min_tier == Tier::Never {
                    return Err(Refusal::GroupHoldsNever(group.into()));
                }
            }
            if !any {
                return Err(Refusal::GroupWithoutRows(group.into()));
            }
        }
        return Ok(Pattern::Group(group.into()));
    }
    if word.len() > MAX_NAME || !plain(word) {
        return Err(shape());
    }
    if agentabi::is_reserved(first_word(word)) {
        let row = agentabi::find(rows, word).ok_or_else(|| Refusal::NoSuchOsRow(word.into()))?;
        if row.min_tier == Tier::Never {
            return Err(Refusal::NeverRow(word.into()));
        }
    }
    Ok(Pattern::Row(word.into()))
}
