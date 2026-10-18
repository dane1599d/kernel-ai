//! The change the kernel shows before it asks to apply an upload: for each file
//! that differs between two source bundles, the lines between the common leading
//! and trailing lines, with up to [`CONTEXT`] lines around them. Lines compare byte
//! for byte, endings included, so a file is never shown as changed on every line
//! only because the checkout used CRLF. Two edits far apart show as one region.

use crate::artifact::source_files;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Unchanged lines shown before and after a region.
pub const CONTEXT: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Changed,
    Added,
    Removed,
}

/// One file's change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange<'a> {
    pub path: &'a str,
    pub kind: Kind,
    /// The first changed line, from 1.
    pub line: usize,
    pub before: Vec<&'a [u8]>,
    pub removed: Vec<&'a [u8]>,
    pub added: Vec<&'a [u8]>,
    pub after: Vec<&'a [u8]>,
}

/// Lines with their endings.
fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|&b| b == b'\n').collect()
}

fn region<'a>(path: &'a str, old: &'a [u8], new: &'a [u8]) -> Option<FileChange<'a>> {
    let (old, new) = (lines(old), lines(new));
    let common = old.len().min(new.len());
    let prefix = (0..common).take_while(|&i| old[i] == new[i]).count();
    let suffix = (0..common - prefix)
        .take_while(|&i| old[old.len() - 1 - i] == new[new.len() - 1 - i])
        .count();
    if prefix == old.len() && prefix == new.len() {
        return None;
    }
    let old_end = old.len() - suffix;
    Some(FileChange {
        path,
        kind: Kind::Changed,
        line: prefix + 1,
        before: old[prefix.saturating_sub(CONTEXT)..prefix].to_vec(),
        removed: old[prefix..old_end].to_vec(),
        added: new[prefix..new.len() - suffix].to_vec(),
        after: old[old_end..(old_end + CONTEXT).min(old.len())].to_vec(),
    })
}

fn whole<'a>(path: &'a str, bytes: &'a [u8], kind: Kind) -> FileChange<'a> {
    let all = lines(bytes);
    let (removed, added) = match kind {
        Kind::Removed => (all, Vec::new()),
        _ => (Vec::new(), all),
    };
    FileChange {
        path,
        kind,
        line: 1,
        before: Vec::new(),
        removed,
        added,
        after: Vec::new(),
    }
}

/// Every file that differs between bundle `old` and bundle `new`: changed files in
/// `old`'s order, then the ones only `new` has.
pub fn changes<'a>(old: &'a [u8], new: &'a [u8]) -> Result<Vec<FileChange<'a>>, &'static str> {
    let (old, new) = (source_files(old)?, source_files(new)?);
    let mut out = Vec::new();
    for &(path, bytes) in &old {
        match new.iter().find(|(p, _)| *p == path) {
            Some(&(_, now)) => out.extend(region(path, bytes, now)),
            None => out.push(whole(path, bytes, Kind::Removed)),
        }
    }
    for &(path, bytes) in &new {
        if !old.iter().any(|(p, _)| *p == path) {
            out.push(whole(path, bytes, Kind::Added));
        }
    }
    Ok(out)
}

/// A line as shown: without its ending, control characters as `?`, tabs as spaces.
pub fn shown(line: &[u8]) -> String {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    String::from_utf8_lossy(line)
        .chars()
        .map(|c| match c {
            '\t' => ' ',
            c if c.is_control() => '?',
            c => c,
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewError {
    /// A bundle does not parse.
    Bundle(&'static str),
    /// More lines than the rows to spare.
    TooLarge,
    /// A line wider than the screen: wrapped, its tail could pass for a line of
    /// its own.
    TooWide,
    /// A line holds bytes that cannot be shown as they are: control characters
    /// (tabs among them), text that is not UTF-8, or characters a terminal could
    /// reorder.
    Unshowable,
}

/// Bytes that cannot be shown as they are in the confirmation, so it refuses them.
fn unshowable(line: &[u8]) -> bool {
    match core::str::from_utf8(line) {
        Err(_) => true,
        Ok(text) => text.chars().any(|c| {
            c.is_control()
                || matches!(
                    c,
                    '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                )
        }),
    }
}

fn without_ending(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// The change as lines for a screen `cols` wide with `rows` rows to spare: a
/// confirmation shows all of what it asks about, exactly, or nothing. It stops at
/// the first line over the budget, so a huge change costs no more than one screen.
/// Empty when nothing changed.
pub fn view(old: &[u8], new: &[u8], cols: usize, rows: usize) -> Result<Vec<String>, ViewError> {
    let changes = changes(old, new).map_err(ViewError::Bundle)?;
    let mut out = Vec::new();
    let mut push = |line: String| {
        if out.len() == rows {
            return Err(ViewError::TooLarge);
        }
        if line.chars().count() > cols {
            return Err(ViewError::TooWide);
        }
        out.push(line);
        Ok(())
    };
    for change in &changes {
        push(header(change))?;
        for (mark, lines) in marked(change) {
            for line in lines {
                let line = without_ending(line);
                if unshowable(line) {
                    return Err(ViewError::Unshowable);
                }
                push(format!("{mark}{}", shown(line)))?;
            }
        }
    }
    Ok(out)
}

fn marked<'c, 'a>(change: &'c FileChange<'a>) -> [(&'static str, &'c Vec<&'a [u8]>); 4] {
    [
        ("  | ", &change.before),
        ("- | ", &change.removed),
        ("+ | ", &change.added),
        ("  | ", &change.after),
    ]
}

/// The changes as lines: a header per file, then its lines marked `  | ` for
/// context, `- | ` removed and `+ | ` added. For the editor's own `diff`.
pub fn render(changes: &[FileChange<'_>]) -> Vec<String> {
    let mut out = Vec::new();
    for change in changes {
        out.push(header(change));
        for (mark, lines) in marked(change) {
            out.extend(lines.iter().map(|line| format!("{mark}{}", shown(line))));
        }
    }
    out
}

fn header(change: &FileChange<'_>) -> String {
    let (removed, added) = (change.removed.len(), change.added.len());
    let path = shown(change.path.as_bytes());
    match change.kind {
        Kind::Added => format!("{path}: added, {added} lines"),
        Kind::Removed => format!("{path}: removed, {removed} lines"),
        Kind::Changed => {
            let endings_only = removed == added
                && change
                    .removed
                    .iter()
                    .zip(&change.added)
                    .all(|(old, new)| without_ending(old) == without_ending(new));
            format!(
                "{path} line {}: {removed} removed, {added} added{}",
                change.line,
                if endings_only {
                    " (line endings only)"
                } else {
                    ""
                }
            )
        }
    }
}

#[cfg(test)]
mod tests;
