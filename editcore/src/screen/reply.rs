//! Reading the edit service's answers back: `view` (`service/view.rs`) and
//! `files`. A reply crosses the kernel and, in an app, an effect's result, so
//! each field is checked, a line out of order ends the reply, and nothing a
//! reply says can make the screen allocate more than the limits below.

use crate::service::view::{MAX_VIEW, MAX_VIEW_CHARS};
use crate::{MAX_FILES, MAX_LINES};
use alloc::string::String;
use alloc::vec::Vec;
use livecore::location;

/// What the file's flags say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    /// The file differs from the opened version.
    pub changed: bool,
    /// There is an edit to undo.
    pub undo: bool,
    /// The text is built.
    pub built: bool,
}

/// A line of a `view` reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub n: usize,
    pub text: String,
    /// The line exactly as the file holds it; false when it was cut or cleaned
    /// and so cannot be edited from the screen.
    pub exact: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub component: String,
    pub label: String,
    pub path: String,
    pub len: usize,
    pub cursor: usize,
    pub flags: Flags,
    pub lines: Vec<Shown>,
}

/// The text after the `ok `, or the reason after the `err `.
pub fn split(reply: &str) -> Result<&str, &str> {
    match reply.split_once(' ') {
        Some(("ok", body)) => Ok(body),
        Some(("err", why)) => Err(why),
        _ if reply == "ok" => Ok(""),
        _ => Err(reply),
    }
}

fn number(text: &str, max: usize) -> Result<usize, String> {
    text.parse()
        .ok()
        .filter(|&n| n <= max)
        .ok_or_else(|| String::from("a bad number in a view"))
}

fn flags(text: &str) -> Result<Flags, String> {
    let mut flags = Flags::default();
    if text == "-" {
        return Ok(flags);
    }
    for c in text.chars() {
        match c {
            'c' => flags.changed = true,
            'u' => flags.undo = true,
            'b' => flags.built = true,
            _ => return Err(String::from("a bad flag in a view")),
        }
    }
    Ok(flags)
}

/// A line's text as the screen may hold it: no control characters but tabs.
fn clean(text: &str) -> (String, bool) {
    let mut exact = true;
    let out = text
        .chars()
        .take(MAX_VIEW_CHARS)
        .map(|c| {
            if c.is_control() && c != '\t' {
                exact = false;
                '?'
            } else {
                c
            }
        })
        .collect();
    (out, exact && text.chars().count() <= MAX_VIEW_CHARS)
}

/// Parses the body of a `view` reply (after `ok `).
pub fn parse_view(body: &str) -> Result<View, String> {
    let mut rows = body.split('\n');
    let head: Vec<&str> = rows.next().unwrap_or("").split(' ').collect();
    let ["view", component, label, path, len, cursor, flags_text] = head[..] else {
        return Err(String::from("not a view"));
    };
    let component = crate::wire::name(component).map_err(String::from)?;
    let label = crate::wire::label(label).map_err(String::from)?;
    if location::shown_path(path) != path || path.starts_with(".../") {
        return Err(String::from("a bad path in a view"));
    }
    let len = number(len, MAX_LINES)?;
    let cursor = number(cursor, MAX_LINES + 1)?;
    let flags = flags(flags_text)?;
    let mut lines = Vec::new();
    for row in rows {
        if lines.len() == MAX_VIEW {
            return Err(String::from("a view with too many lines"));
        }
        let split = row
            .find([':', '~'])
            .ok_or_else(|| String::from("a view line with no number"))?;
        let n = number(&row[..split], len)?;
        if n == 0 || lines.last().is_some_and(|last: &Shown| n <= last.n) {
            return Err(String::from("view lines out of order"));
        }
        let (text, exact) = clean(&row[split + 1..]);
        let exact = exact && row.as_bytes()[split] == b':';
        lines.push(Shown { n, text, exact });
    }
    Ok(View {
        component: String::from(component),
        label: String::from(label),
        path: String::from(path),
        len,
        cursor,
        flags,
        lines,
    })
}

/// A file of the session, from `files`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub len: usize,
    pub changed: bool,
    pub current: bool,
}

/// Parses the body of a `files` reply: `N files; * changed, > current`, then a
/// line each: two mark characters, a space, the path and `(N lines)`.
pub fn parse_files(body: &str) -> Result<Vec<Entry>, String> {
    let mut rows = body.split('\n');
    let head = rows.next().unwrap_or("");
    let count = head
        .split_once(" files;")
        .and_then(|(n, _)| n.parse::<usize>().ok())
        .filter(|&n| n > 0 && n <= MAX_FILES)
        .ok_or_else(|| String::from("not a file list"))?;
    let mut files = Vec::new();
    for row in rows.take(MAX_FILES) {
        let bad = || String::from("a bad line in a file list");
        let (marks, rest) = row.split_at_checked(2).ok_or_else(bad)?;
        let rest = rest.strip_prefix(' ').ok_or_else(bad)?;
        let (path, tail) = rest.rsplit_once(" (").ok_or_else(bad)?;
        let len = tail
            .strip_suffix(" lines)")
            .and_then(|n| n.parse().ok())
            .filter(|&n| n <= MAX_LINES)
            .ok_or_else(bad)?;
        if location::shown_path(path) != path {
            return Err(bad());
        }
        files.push(Entry {
            path: String::from(path),
            len,
            changed: marks.starts_with('*'),
            current: marks.ends_with('>'),
        });
    }
    if files.len() != count {
        return Err(String::from("a file list that is not as long as it says"));
    }
    Ok(files)
}
