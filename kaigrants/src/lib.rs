//! `kai-grants 1` (ADR 0001 D8): grant and standing-rule records, one line each,
//! keys in a fixed order.
//!
//! ```text
//! kai-grants 1
//! grant 41 app=notes kind=dir object=/home/user/notes rights=read+write+enumerate from=picker life=persistent signer=- made=2026-11-02T10:14:00Z
//! rule 57 action=proc_stop target=build-3 from=prompt:212 made=2026-11-02T10:20:00Z
//! ```
//!
//! The text comes back from disk, so it is read as hostile: an unknown, missing
//! or reordered key, a line kind it does not know, an id used twice or any value
//! out of shape refuses the whole file, with its line. A new key means
//! `kai-grants 2`. A record holds no handle; whoever reads one makes its node again.
//!
//! This crate is `no_std` + `alloc`: the shell, `powerbox` later and the host
//! tests run the same code.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod time;

#[cfg(test)]
mod tests;

pub use time::Time;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;

/// The first line.
pub const HEADER: &str = "kai-grants 1";
/// Most records a file may hold.
pub const MAX_RECORDS: usize = 1024;
/// Longest line, so a hostile file cannot make one value huge.
pub const MAX_LINE: usize = 512;

/// One grant: what `app` may reach, and how it came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    /// Never reused; generations name grants by it (ADR 0011).
    pub id: u32,
    pub app: String,
    pub kind: Kind,
    /// An absolute path, or a host for [`Kind::Host`].
    pub object: String,
    /// Rights words, as `read+write+enumerate`.
    pub rights: Vec<String>,
    pub from: From,
    pub life: Life,
    /// The SHA-256 of the package signer's key; `None` until P8.
    pub signer: Option<[u8; 32]>,
    pub made: Time,
}

/// A standing rule: "Always allow" on the ask menu (ADR 0002 D2a).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub id: u32,
    /// A registry action name.
    pub action: String,
    /// A pattern for the row's `Target` argument; `None` for every call.
    pub target: Option<String>,
    /// The log entry whose menu made it (`from=prompt:<n>`).
    pub prompt: u64,
    pub made: Time,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Host,
}

/// Where a grant came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum From {
    Install,
    Picker,
    /// The log entry of the request that asked for it.
    Request(u64),
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Life {
    Persistent,
    /// Dropped at boot.
    Session,
}

/// A whole `kai-grants 1` file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    pub grants: Vec<Grant>,
    pub rules: Vec<Rule>,
}

/// Why a file was refused, and on which line (1 is the header).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub why: String,
}

impl Grants {
    /// The file's text: the header, then grants, then rules, each in id order as
    /// held. Refuses a record that would not read back.
    pub fn text(&self) -> Result<String, String> {
        let mut out = String::from(HEADER);
        out.push('\n');
        for g in &self.grants {
            let signer = g.signer.map_or(String::from("-"), |s| hex(&s));
            let line = format!(
                "grant {} app={} kind={} object={} rights={} from={} life={} signer={signer} made={}",
                g.id,
                g.app,
                g.kind.word(),
                g.object,
                g.rights.join("+"),
                g.from.word(),
                g.life.word(),
                g.made.format()
            );
            push_line(&mut out, line)?;
        }
        for r in &self.rules {
            let line = format!(
                "rule {} action={} target={} from=prompt:{} made={}",
                r.id,
                r.action,
                r.target.as_deref().unwrap_or("-"),
                r.prompt,
                r.made.format()
            );
            push_line(&mut out, line)?;
        }
        // What is written must read back the same.
        match parse(&out) {
            Ok(read) if read == *self => Ok(out),
            Ok(_) => Err(String::from("a record does not read back as written")),
            Err(e) => Err(e.why),
        }
    }

    /// What a boot keeps: `session` grants are dropped.
    pub fn at_boot(mut self) -> Self {
        self.grants.retain(|g| g.life == Life::Persistent);
        self
    }
}

fn push_line(out: &mut String, line: String) -> Result<(), String> {
    if line.len() > MAX_LINE {
        return Err(format!("a record is longer than {MAX_LINE} bytes"));
    }
    let _ = writeln!(out, "{line}");
    Ok(())
}

/// Reads a whole file, or refuses it.
pub fn parse(text: &str) -> Result<Grants, Error> {
    let mut lines = text.split_inclusive('\n');
    let at = |line: usize| {
        move |why: &str| Error {
            line,
            why: why.to_string(),
        }
    };
    if lines.next() != Some("kai-grants 1\n") {
        return Err(at(1)("the file does not start with kai-grants 1"));
    }
    let mut grants = Grants::default();
    let mut ids = Vec::new();
    for (i, raw) in lines.enumerate() {
        let n = i + 2;
        let line = raw
            .strip_suffix('\n')
            .ok_or_else(|| at(n)("the last line has no newline"))?;
        if line.len() > MAX_LINE {
            return Err(at(n)("the line is too long"));
        }
        if ids.len() >= MAX_RECORDS {
            return Err(at(n)("there are too many records"));
        }
        let (id, record) = record(line).map_err(at(n))?;
        if ids.contains(&id) {
            return Err(at(n)("an id is used twice"));
        }
        ids.push(id);
        match record {
            Record::Grant(g) => grants.grants.push(g),
            Record::Rule(r) => grants.rules.push(r),
        }
    }
    Ok(grants)
}

enum Record {
    Grant(Grant),
    Rule(Rule),
}

const GRANT_KEYS: [&str; 8] = [
    "app", "kind", "object", "rights", "from", "life", "signer", "made",
];
const RULE_KEYS: [&str; 4] = ["action", "target", "from", "made"];

fn record(line: &str) -> Result<(u32, Record), &'static str> {
    let mut words = line.split(' ');
    let kind = words.next().unwrap_or("");
    let id = number(words.next().unwrap_or(""))?;
    let id = u32::try_from(id).map_err(|_| "an id is over u32")?;
    let values: Vec<&str> = words.collect();
    let keys: &[&str] = match kind {
        "grant" => &GRANT_KEYS,
        "rule" => &RULE_KEYS,
        _ => return Err("a line is neither a grant nor a rule"),
    };
    if values.len() != keys.len() {
        return Err("a record has the wrong number of keys");
    }
    let mut v = Vec::with_capacity(keys.len());
    for (word, key) in values.iter().zip(keys) {
        let value = word
            .strip_prefix(key)
            .and_then(|rest| rest.strip_prefix('='))
            .ok_or("a key is unknown or out of order")?;
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_graphic() && b != b'=') {
            return Err("a value is empty or out of shape");
        }
        v.push(value);
    }
    let made = Time::parse(v[keys.len() - 1])?;
    let record = match kind {
        "grant" => Record::Grant(Grant {
            id,
            app: name(v[0])?,
            kind: Kind::parse(v[1])?,
            object: object(Kind::parse(v[1])?, v[2])?,
            rights: rights(v[3])?,
            from: From::parse(v[4])?,
            life: Life::parse(v[5])?,
            signer: signer(v[6])?,
            made,
        }),
        _ => Record::Rule(Rule {
            id,
            action: name(v[0])?,
            target: match v[1] {
                "-" => None,
                t => Some(t.to_string()),
            },
            prompt: number(
                v[2].strip_prefix("prompt:")
                    .ok_or("a rule is not from=prompt:<n>")?,
            )?,
            made,
        }),
    };
    Ok((id, record))
}

/// A decimal with no sign, no leading zero and no more than u64 holds.
fn number(text: &str) -> Result<u64, &'static str> {
    let canonical = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    match canonical {
        true => text.parse().map_err(|_| "a number is too large"),
        false => Err("a number is out of shape"),
    }
}

/// A package or action name: lowercase letters, digits, `-` and `_`.
fn name(text: &str) -> Result<String, &'static str> {
    let ok = text
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    ok.then(|| text.to_string()).ok_or("a name is out of shape")
}

/// An absolute path with no `.` or `..` part, or for a host grant a host name.
fn object(kind: Kind, text: &str) -> Result<String, &'static str> {
    let ok = match kind {
        Kind::Host => text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'),
        Kind::Dir | Kind::File => {
            text.starts_with('/') && !text[1..].split('/').any(|p| matches!(p, "" | "." | ".."))
        }
    };
    ok.then(|| text.to_string())
        .ok_or("an object is out of shape")
}

fn rights(text: &str) -> Result<Vec<String>, &'static str> {
    text.split('+')
        .map(
            |w| match !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase()) {
                true => Ok(w.to_string()),
                false => Err("a right is out of shape"),
            },
        )
        .collect()
}

fn signer(text: &str) -> Result<Option<[u8; 32]>, &'static str> {
    if text == "-" {
        return Ok(None);
    }
    let b = text.as_bytes();
    if b.len() != 64 {
        return Err("a signer is not 64 hex digits");
    }
    let mut out = [0u8; 32];
    for (i, pair) in b.chunks(2).enumerate() {
        let digit = |c: u8| match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            _ => Err("a signer is not lowercase hex"),
        };
        out[i] = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Ok(Some(out))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Dir => "dir",
            Kind::File => "file",
            Kind::Host => "host",
        }
    }

    fn parse(text: &str) -> Result<Self, &'static str> {
        match text {
            "dir" => Ok(Kind::Dir),
            "file" => Ok(Kind::File),
            "host" => Ok(Kind::Host),
            _ => Err("a kind is not dir, file or host"),
        }
    }
}

impl From {
    fn word(self) -> String {
        match self {
            From::Install => String::from("install"),
            From::Picker => String::from("picker"),
            From::Request(n) => format!("request:{n}"),
            From::Settings => String::from("settings"),
        }
    }

    fn parse(text: &str) -> Result<Self, &'static str> {
        match text {
            "install" => Ok(From::Install),
            "picker" => Ok(From::Picker),
            "settings" => Ok(From::Settings),
            _ => match text.strip_prefix("request:") {
                Some(n) => number(n).map(From::Request),
                None => Err("a grant's from is unknown"),
            },
        }
    }
}

impl Life {
    fn word(self) -> &'static str {
        match self {
            Life::Persistent => "persistent",
            Life::Session => "session",
        }
    }

    fn parse(text: &str) -> Result<Self, &'static str> {
        match text {
            "persistent" => Ok(Life::Persistent),
            "session" => Ok(Life::Session),
            _ => Err("a life is not persistent or session"),
        }
    }
}
