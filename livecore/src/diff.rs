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
    /// The view needs this many rows.
    TooLarge(usize),
}

/// The change as lines for a screen `cols` wide with `rows` rows to spare, or
/// TooLarge when it does not fit: a confirmation must show all of what it asks
/// about. Empty when nothing changed.
pub fn view(old: &[u8], new: &[u8], cols: usize, rows: usize) -> Result<Vec<String>, ViewError> {
    let changes = changes(old, new).map_err(ViewError::Bundle)?;
    let out = render(&changes);
    let needed = rows_needed(&out, cols);
    if needed > rows {
        return Err(ViewError::TooLarge(needed));
    }
    Ok(out)
}

/// The changes as lines: a header per file, then its lines marked `  | ` for
/// context, `- | ` removed and `+ | ` added.
pub fn render(changes: &[FileChange<'_>]) -> Vec<String> {
    let mut out = Vec::new();
    for change in changes {
        out.push(header(change));
        let marked = [
            ("  | ", &change.before),
            ("- | ", &change.removed),
            ("+ | ", &change.added),
            ("  | ", &change.after),
        ];
        for (mark, lines) in marked {
            out.extend(lines.iter().map(|line| format!("{mark}{}", shown(line))));
        }
    }
    out
}

/// Screen rows `lines` take `cols` wide, counting the rows long lines wrap to.
pub fn rows_needed(lines: &[String], cols: usize) -> usize {
    let cols = cols.max(1);
    lines
        .iter()
        .map(|line| line.chars().count().div_ceil(cols).max(1))
        .sum()
}

fn header(change: &FileChange<'_>) -> String {
    let (removed, added) = (change.removed.len(), change.added.len());
    match change.kind {
        Kind::Added => format!("{}: added, {added} lines", change.path),
        Kind::Removed => format!("{}: removed, {removed} lines", change.path),
        Kind::Changed => {
            let endings_only = removed == added
                && change
                    .removed
                    .iter()
                    .zip(&change.added)
                    .all(|(old, new)| shown(old) == shown(new));
            format!(
                "{} line {}: {removed} removed, {added} added{}",
                change.path,
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
