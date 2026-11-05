//! One memory, and `kai-memory 1`: a header line with the next id, then one
//! record per line pair, the fields and then the text. The next id is kept so
//! a forgotten memory's id is never handed out again (C on #650).
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

/// A `kai-memory 1` file's next id and memories, or why it is not one.
/// Strict: a bad record, two with one id, or an id not below the next one
/// refuses the whole file.
pub fn decode(file: &str) -> Result<(u64, Vec<Memory>), String> {
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
    let mut memories: Vec<Memory> = Vec::new();
    let mut at = 2;
    while let Some(fields) = lines.next() {
        let text = lines
            .next()
            .ok_or_else(|| format!("line {at}: a record without its text"))?;
        let memory =
            Memory::parse(fields, text).ok_or_else(|| format!("line {at}: a bad record"))?;
        if memories.iter().any(|m| m.id == memory.id) {
            return Err(format!("line {at}: id {} twice", memory.id));
        }
        if memory.id >= next {
            return Err(format!(
                "line {at}: id {} is not below next={next}",
                memory.id
            ));
        }
        memories.push(memory);
        at += 2;
    }
    Ok((next, memories))
}
