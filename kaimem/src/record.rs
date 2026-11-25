//! One memory, and `kai-memory 1`: a header line with the next id, then one
//! record per line pair, the fields and then the text. The next id is kept so
//! a forgotten memory's id is never handed out again (C on #650).
//!
//! A record's fields start with `id=` and a memory's text never does, so a
//! record starts at every line that starts with `id=`. A bad record is skipped
//! and reported ([`Skipped`]), and the records around it are read; a bad
//! header or another version refuses the whole file.
//!
//! ```text
//! kai-memory 1 next=4
//! id=3 time=1762300000000000000 source=41 scope=all
//! I use metric units
//! ```

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The file's first line.
pub const HEADER: &str = "kai-memory 1";
/// The longest text a memory may hold, in bytes.
pub const MAX_TEXT: usize = 200;
/// How every record's fields line starts, and no memory's text may.
pub const RECORD_START: &str = "id=";
/// The most memories kept (est.): a store this full refuses new ones until
/// some are forgotten, and a file holding more reads only this many.
pub const MAX_MEMORIES: usize = 512;

/// Who a memory is for: every agent, or one agent's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    Agent(String),
}

impl Scope {
    fn word(&self) -> &str {
        match self {
            Scope::All => "all",
            Scope::Agent(name) => name,
        }
    }

    fn parse(word: &str) -> Option<Scope> {
        match word {
            "all" => Some(Scope::All),
            name if is_agent_name(name) => Some(Scope::Agent(name.to_string())),
            _ => None,
        }
    }
}

/// An agent's name, ADR 0011's rule: `[a-z][a-z0-9-]{1,11}`.
pub fn is_agent_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (2..=12).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// One memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    pub id: u64,
    /// When it was made or last edited, in ns since the epoch.
    pub time: u64,
    /// The `actlog` entry that made it.
    pub source: u64,
    pub scope: Scope,
    pub text: String,
}

/// Characters that change how text shows without showing themselves: the
/// bidi controls and the zero-width ones (C on #650), so a question cannot
/// show text other than what is kept.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// Why a text cannot be a memory, or `None` when it can: not empty, at most
/// [`MAX_TEXT`] bytes, one line of characters that show, and no permission or
/// standing action ([`crate::directive`]). Every way a memory is made checks
/// it: `remember`, `edit` and a file read back (C on #652).
pub fn text_problem(text: &str) -> Option<&'static str> {
    if text.trim().is_empty() {
        return Some("a memory needs some text");
    }
    if text.len() > MAX_TEXT {
        return Some("a memory is at most 200 bytes");
    }
    if text.chars().any(|c| c.is_control() || invisible(c)) {
        return Some("a memory is one line of printable text");
    }
    // The file finds its records by this start, so a text that had it would read
    // back as a record of its own.
    if text.starts_with(RECORD_START) {
        return Some("a memory cannot start with id=");
    }
    crate::directive::problem(text)
}

impl Memory {
    fn fields(&self) -> String {
        format!(
            "id={} time={} source={} scope={}",
            self.id,
            self.time,
            self.source,
            self.scope.word()
        )
    }

    fn parse(fields: &str, text: &str) -> Option<Memory> {
        let mut words = fields.split(' ');
        let mut field = |key: &str| words.next()?.strip_prefix(key)?.strip_prefix('=');
        let id = field("id")?.parse().ok()?;
        let time = field("time")?.parse().ok()?;
        let source = field("source")?.parse().ok()?;
        let scope = Scope::parse(field("scope")?)?;
        if words.next().is_some() || text_problem(text).is_some() {
            return None;
        }
        Some(Memory {
            id,
            time,
            source,
            scope,
            text: text.to_string(),
        })
    }
}

/// `memories` as a `kai-memory 1` file, with `next`, the id the next memory
/// gets.
pub fn encode(next: u64, memories: &[Memory]) -> String {
    let mut out = format!("{HEADER} next={next}\n");
    for memory in memories {
        out.push_str(&memory.fields());
        out.push('\n');
        out.push_str(&memory.text);
        out.push('\n');
    }
    out
}

/// Why a record was skipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// Lines before the first record that are no part of one.
    Stray,
    /// Fields with no text line after them.
    NoText,
    /// Fields with more than one line after them.
    ExtraLines,
    /// Fields out of shape: a missing, unknown or extra field, or a bad value.
    BadFields,
    /// A text that cannot be a memory ([`text_problem`]).
    BadText,
    /// An id an earlier record has.
    IdTwice(u64),
    /// An id the header's next one has not passed: it would be handed out again.
    IdNotBelowNext(u64),
    /// A text its scope already keeps (the store's rule: nothing kept twice).
    SameText(u64),
    /// A record past the most the store keeps.
    Full(u64),
}

/// A record left out of what was read: its lines, 1-based, and why. It is
/// dropped from the file at the next save, so the caller says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub first: usize,
    pub last: usize,
    pub why: Why,
}

impl core::fmt::Display for Skipped {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.first == self.last {
            true => write!(f, "line {}: ", self.first)?,
            false => write!(f, "lines {}-{}: ", self.first, self.last)?,
        }
        match self.why {
            Why::Stray => f.write_str("not part of a record"),
            Why::NoText => f.write_str("a record without its text"),
            Why::ExtraLines => f.write_str("a record with more than one line of text"),
            Why::BadFields => f.write_str("a record whose fields are out of shape"),
            Why::BadText => f.write_str("a record whose text cannot be a memory"),
            Why::IdTwice(id) => write!(f, "id {id} twice"),
            Why::IdNotBelowNext(id) => write!(f, "id {id} is not below the next id"),
            Why::SameText(id) => write!(f, "id {id} says what an earlier memory says"),
            Why::Full(id) => write!(f, "id {id} is past the most memories kept"),
        }?;
        f.write_str("; skipped")
    }
}

/// What a `kai-memory 1` file holds: the next id, the memories, and every
/// record left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub next: u64,
    pub memories: Vec<Memory>,
    pub skipped: Vec<Skipped>,
}

/// A `kai-memory 1` file's next id and memories, or why it is not one. A bad
/// header or another version refuses the whole file. A bad record, a record
/// with an id an earlier one has, one whose id is not below the next one, one
/// whose text its scope already keeps, and one past [`MAX_MEMORIES`] are
/// skipped and reported, and the rest is read, so what is read keeps the
/// store's rules.
pub fn decode(file: &str) -> Result<Decoded, String> {
    let mut lines = file.lines();
    let next = lines
        .next()
        .and_then(|line| {
            line.strip_prefix(HEADER)?
                .strip_prefix(" next=")?
                .parse::<u64>()
                .ok()
        })
        .ok_or_else(|| String::from("not a kai-memory 1 file"))?;
    let mut read = Decoded {
        next,
        memories: Vec::new(),
        skipped: Vec::new(),
    };
    // Each record is its fields line and every line up to the next record.
    let mut record: Option<(usize, &str, Vec<&str>)> = None;
    let mut stray: Option<(usize, usize)> = None;
    for (at, line) in (2..).zip(lines) {
        if line.starts_with(RECORD_START) {
            if let Some(done) = record.replace((at, line, Vec::new())) {
                read.take(done);
            }
        } else if let Some((_, _, text)) = record.as_mut() {
            text.push(line);
        } else {
            let first = stray.map_or(at, |(first, _)| first);
            stray = Some((first, at));
        }
    }
    if let Some((first, last)) = stray {
        read.skip(first, last, Why::Stray);
    }
    if let Some(done) = record {
        read.take(done);
    }
    read.skipped.sort_by_key(|s| s.first);
    Ok(read)
}

impl Decoded {
    fn skip(&mut self, first: usize, last: usize, why: Why) {
        self.skipped.push(Skipped { first, last, why });
    }

    /// One record, from line `at`: kept, or skipped with why.
    fn take(&mut self, (at, fields, text): (usize, &str, Vec<&str>)) {
        let last = at + text.len();
        let why = match text.as_slice() {
            [] => Why::NoText,
            [_, _, ..] => Why::ExtraLines,
            [text] => match Memory::parse(fields, text) {
                None if Memory::parse(fields, "x").is_none() => Why::BadFields,
                None => Why::BadText,
                Some(memory) if self.memories.iter().any(|m| m.id == memory.id) => {
                    Why::IdTwice(memory.id)
                }
                Some(memory) if memory.id >= self.next => Why::IdNotBelowNext(memory.id),
                Some(memory)
                    if self.memories.iter().any(|m| {
                        m.scope == memory.scope && m.text.eq_ignore_ascii_case(&memory.text)
                    }) =>
                {
                    Why::SameText(memory.id)
                }
                Some(memory) if self.memories.len() >= MAX_MEMORIES => Why::Full(memory.id),
                Some(memory) => {
                    self.memories.push(memory);
                    return;
                }
            },
        };
        self.skip(at, last, why);
    }
}
