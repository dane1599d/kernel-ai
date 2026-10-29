//! Source links: `path:line` and `path:line:column` inside any text a screen
//! shows (a compiler's `--> src/lib.rs:5:9`, a fault's `panicked at ...`, a
//! `find` hit), found so a Ctrl-click or Enter can open that line.
//!
//! The text is hostile: a log line, a file's own comment. A link names only a
//! plain relative path of at most [`location::MAX_PATH`] bytes (the rule that
//! keeps host paths out of logs), with a file extension, and a line and column
//! no source here exceeds. Nothing else is a link, and the scan reads at most
//! [`MAX_SCAN`] bytes of a text and finds at most [`MAX_LINKS`] in it.

use alloc::string::String;
use alloc::vec::Vec;
use livecore::location;

/// Most bytes of a text the scan reads.
pub const MAX_SCAN: usize = 1024;
/// Most links one text yields.
pub const MAX_LINKS: usize = 8;
/// Most digits in a line number or a column.
const LINE_DIGITS: usize = 5;
const COLUMN_DIGITS: usize = 4;

/// A place in a source file, and where its words are in the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// The link's first byte in the text.
    pub start: usize,
    /// One past its last byte.
    pub end: usize,
    pub path: String,
    pub line: u32,
    /// From 1; 0 when the text gave none.
    pub column: u32,
}

fn path_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'/' | b'-')
}

/// `text[at..]`'s leading decimal digits, as a number, and how many there were;
/// None when there are none or more than `max`.
fn digits(text: &[u8], at: usize, max: usize) -> Option<(u32, usize)> {
    let run = text
        .get(at..)?
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    if run == 0 || run > max {
        return None;
    }
    let value = text[at..at + run]
        .iter()
        .fold(0u32, |n, &b| n * 10 + u32::from(b - b'0'));
    Some((value, run))
}

/// Whether a run of path bytes is a file a link may name.
fn file_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or("");
    let extension = name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && ext.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
            && ext.bytes().all(|b| b.is_ascii_alphanumeric())
    });
    extension && !path.starts_with('/') && location::shown_path(path) == path
}

/// The link whose path starts at `start`, if one does.
fn link_at(text: &[u8], start: usize) -> Option<Link> {
    let end = start + text[start..].iter().take_while(|&&b| path_byte(b)).count();
    if text.get(end) != Some(&b':') {
        return None;
    }
    let path = core::str::from_utf8(&text[start..end]).ok()?;
    if !file_path(path) {
        return None;
    }
    let (line, used) = digits(text, end + 1, LINE_DIGITS)?;
    let mut stop = end + 1 + used;
    let mut column = 0;
    if text.get(stop) == Some(&b':')
        && let Some((value, used)) = digits(text, stop + 1, COLUMN_DIGITS)
        && !word_byte(text.get(stop + 1 + used))
    {
        column = value;
        stop += 1 + used;
    }
    if !(1..=99_999).contains(&line) || word_byte(text.get(stop)) {
        return None;
    }
    Some(Link {
        start,
        end: stop,
        path: String::from(path),
        line,
        column,
    })
}

/// A byte that would make a number or a path part of a longer word.
fn word_byte(byte: Option<&u8>) -> bool {
    byte.is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The links in `text`, in order.
pub fn scan(text: &str) -> Vec<Link> {
    let all = text.as_bytes();
    let mut limit = all.len().min(MAX_SCAN);
    if all.len() > MAX_SCAN {
        // The cut may fall inside a number; a link must end before it.
        while limit > 0 && (path_byte(all[limit - 1]) || all[limit - 1] == b':') {
            limit -= 1;
        }
    }
    let bytes = &all[..limit];
    let mut links = Vec::new();
    let mut at = 0;
    while at < bytes.len() && links.len() < MAX_LINKS {
        if !path_byte(bytes[at]) {
            at += 1;
            continue;
        }
        // A path starts where a run of path bytes does, never inside one.
        let run = bytes[at..].iter().take_while(|&&b| path_byte(b)).count();
        match link_at(bytes, at) {
            Some(link) => {
                at = link.end;
                links.push(link);
            }
            None => at += run,
        }
    }
    links
}

/// The link whose words cover character `col` of `text`.
pub fn at_column(text: &str, col: usize) -> Option<Link> {
    let byte = text.char_indices().nth(col).map(|(at, _)| at)?;
    scan(text)
        .into_iter()
        .find(|link| (link.start..link.end).contains(&byte))
}
