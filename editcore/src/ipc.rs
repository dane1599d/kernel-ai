//! What the kernel sends the edit service, one IPC message each, and how the
//! kernel reads the service's answers: a sync's, and the line a reply begins with
//! when a build moved the cursor.

use crate::wire::{Reader, SyncRequest, label, name, read_sync, sync_fields};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::live::valid_commit;
use livecore::location;

/// A line in a session's source: a plain workspace path, and a line from 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct At<'a> {
    pub path: &'a str,
    pub line: u32,
}

/// What the kernel sends the edit service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ipc<'a> {
    /// Open (or resume) a session on a version, the active one by default, with
    /// the cursor at a line if one is given.
    Open {
        component: &'a str,
        label: Option<&'a str>,
        at: Option<At<'a>>,
    },
    /// One line typed in edit mode.
    Cmd(&'a str),
    /// Record outbox job `seq` in git.
    Sync { seq: u64, job: SyncRequest<'a> },
    /// Journal entries from `from` on, one a line, for a restarted service.
    Replay { from: usize, entries: &'a str },
}

/// `open <component> [<label>] [at <path>:<line>]`, `cmd <line>`, `sync <seq>`
/// and the job's fields, or `replay <from>` and the entries.
pub fn encode_ipc(message: &Ipc<'_>) -> Vec<u8> {
    match message {
        Ipc::Open {
            component,
            label,
            at,
        } => {
            let mut out = format!("open {component}");
            if let Some(label) = label {
                out.push_str(&format!(" {label}"));
            }
            if let Some(at) = at {
                out.push_str(&format!(" at {}:{}", at.path, at.line));
            }
            out.into_bytes()
        }
        Ipc::Cmd(line) => format!("cmd {line}").into_bytes(),
        Ipc::Sync { seq, job } => format!("sync {seq}\n{}", sync_fields(job)).into_bytes(),
        Ipc::Replay { from, entries } => format!("replay {from}\n{entries}").into_bytes(),
    }
}

fn at(text: &str) -> Result<At<'_>, &'static str> {
    let (path, line) = text.rsplit_once(':').ok_or("a place has no line")?;
    let line = line.parse().ok().filter(|&n| n > 0).ok_or("a bad line")?;
    if path.starts_with(".../") || location::shown_path(path) != path {
        return Err("a path that is not plain");
    }
    Ok(At { path, line })
}

pub fn decode_ipc(bytes: &[u8]) -> Result<Ipc<'_>, &'static str> {
    let text = core::str::from_utf8(bytes).map_err(|_| "not text")?;
    if let Some(rest) = text.strip_prefix("cmd ") {
        return Ok(Ipc::Cmd(rest));
    }
    if text == "cmd" {
        return Ok(Ipc::Cmd(""));
    }
    if let Some(rest) = text.strip_prefix("open ") {
        let words: Vec<&str> = rest.split(' ').collect();
        let (component, label_text, at_text) = match words[..] {
            [component] => (component, None, None),
            [component, "at", place] => (component, None, Some(place)),
            [component, label] => (component, Some(label), None),
            [component, label, "at", place] => (component, Some(label), Some(place)),
            _ => return Err("open takes a component, a label and a place"),
        };
        return Ok(Ipc::Open {
            component: name(component)?,
            label: label_text.map(label).transpose()?,
            at: at_text.map(at).transpose()?,
        });
    }
    if let Some(rest) = text.strip_prefix("sync ") {
        let (seq, fields) = rest.split_once('\n').ok_or("a sync has no fields")?;
        let seq = seq.parse().map_err(|_| "the job number is not a number")?;
        let mut reader = Reader {
            rest: fields.as_bytes(),
        };
        let job = read_sync(&mut reader)?;
        reader.end()?;
        return Ok(Ipc::Sync { seq, job });
    }
    if let Some(rest) = text.strip_prefix("replay ") {
        let (from, entries) = rest.split_once('\n').ok_or("a replay has no entries")?;
        let from = from
            .parse()
            .map_err(|_| "the entry number is not a number")?;
        return Ok(Ipc::Replay { from, entries });
    }
    Err("an unknown request")
}

/// Whether an app that holds the `edit` grant may send `message` to the edit
/// service: an open, or a command of one line. A sync or a replay is the
/// kernel's alone, and `try` is left to the kernel's editor, since it can take a
/// minute with the caller waiting.
pub fn for_app(message: &[u8]) -> Result<(), &'static str> {
    match decode_ipc(message)? {
        Ipc::Open { .. } => Ok(()),
        Ipc::Cmd(line) if line.chars().any(|c| c.is_control() && c != '\t') => {
            Err("a command holds a control character")
        }
        Ipc::Cmd(line) if line.len() > crate::journal::MAX_COMMAND => {
            Err("a command too long to journal")
        }
        Ipc::Cmd(line) if line.split(' ').next() == Some("try") => {
            Err("try is for the kernel's editor")
        }
        Ipc::Cmd(_) => Ok(()),
        _ => Err("only an open or a command"),
    }
}

/// The edit service's answer to a sync of job `seq`: the commit git recorded, or
/// empty when git tracks no commit for it. Anything else is an error to retry.
pub fn parse_ack(reply: &[u8], seq: u64) -> Result<String, String> {
    let reply =
        core::str::from_utf8(reply).map_err(|_| String::from("a reply that is not text"))?;
    let mut words = reply.split(' ');
    match (
        words.next(),
        words.next(),
        words.next(),
        words.next(),
        words.next(),
    ) {
        (Some("ok"), Some("synced"), Some(n), Some(sha), None)
            if n.parse() == Ok(seq) && valid_commit(sha) =>
        {
            Ok(String::from(sha))
        }
        (Some("ok"), Some("untracked"), Some(n), None, None) if n.parse() == Ok(seq) => {
            Ok(String::new())
        }
        _ => Err(String::from(reply)),
    }
}

/// How a reply begins when a build moved the cursor since the last open or
/// command: `#at <path>:<line>` on a line of its own. Replies proper begin `ok `
/// or `err `, so nothing else begins this way.
pub const MOVED_MARK: &str = "#at ";
/// Room every reply leaves for that line, so a reply replayed from the journal is
/// cut where the live one was.
pub const MARK_ROOM: usize = 96;

/// `reply`, begun with the place a build moved the cursor to.
pub fn with_moved(place: &str, reply: &str) -> String {
    format!("{MOVED_MARK}{place}\n{reply}")
}

/// The place a build moved the cursor to, when the reply begins with one, and the
/// reply without it; any other reply comes back whole.
pub fn split_moved(reply: &[u8]) -> (Option<&str>, &[u8]) {
    let Some(rest) = reply.strip_prefix(MOVED_MARK.as_bytes()) else {
        return (None, reply);
    };
    let Some(end) = rest.iter().position(|&byte| byte == b'\n') else {
        return (None, reply);
    };
    match core::str::from_utf8(&rest[..end]) {
        Ok(place) => (Some(place), &rest[end + 1..]),
        Err(_) => (None, reply),
    }
}
