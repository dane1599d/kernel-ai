//! `kai-cc 1` (ADR 0012 D11): what the shell and `cc` say to each other, one
//! request per message and one reply per message.
//!
//! ```text
//! shell to cc                         cc to shell
//! open <session> origin=user|model    out <n>\n<n bytes>         printed output
//! eval <seq>\n<entry>                 def <kind> <slot>|- <n>\n<name>
//! replay <seq>\n<entry>               retyped <function>          old callers keep the old body
//! resume <id> ok|err\n<result>        value <type> <n>\n<text>    the shown value
//! undo                                -- and then exactly one of --
//! dis <name>                          need <id> <n>\n<call>       an effect, as ADR 0002's call
//! defs                                done <compile_us> <link_us> <run_us>
//! reset                               err <line> <n>\n<text>      line 0: not in the entry
//! build <component> <version>         trap <reason> <function>|-
//!                                     undone <seq>
//!                                     text <n>\n<text>            for dis and defs
//!                                     ok
//! ```
//!
//! `build` comes with an `Shm` that holds the component's source bundle; cc
//! writes the artifact over it and answers `text` (see `ccserve::build::shm`).
//!
//! Every reply header ends in a newline; a record that carries text gives its
//! byte count last, and the bytes follow the newline. So output and wcc's
//! messages may hold any byte, newlines included, and nothing in them can start
//! a record. D11's `done <type> <value> …` is split into `value` and `done` for
//! the same reason. A `need` carries an id of its own rather than the entry's
//! seq, so a late or repeated `resume` cannot answer the entry's next effect.
//!
//! Both sides read the other's messages as hostile: a word out of shape, a count
//! past the end, a second terminal record or bytes after it refuse the message.

use crate::MAX_ENTRY;
use crate::defs::Kind;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Longest header line, its newline excluded.
pub const MAX_HEADER: usize = 128;
/// Most records in one reply.
pub const MAX_RECORDS: usize = 512;
/// Longest definition name in a `def` record.
pub const MAX_NAME: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireError(pub &'static str);

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "kai-cc 1: {}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    User,
    Model,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request<'a> {
    /// Starts a session, dropping any other.
    Open {
        session: &'a str,
        origin: Origin,
    },
    Eval {
        seq: u64,
        entry: &'a str,
    },
    /// An entry's definitions only, after a restart (D4).
    Replay {
        seq: u64,
        entry: &'a str,
    },
    /// The answer to `need <id>`.
    Resume {
        id: u64,
        ok: bool,
        result: &'a str,
    },
    Undo,
    Dis(&'a str),
    Defs,
    Reset,
    /// `cc build`: the sources come in an `Shm`, not in the message.
    Build {
        component: &'a str,
        version: &'a str,
    },
}

impl<'a> Request<'a> {
    pub fn parse(msg: &'a str) -> Result<Self, WireError> {
        let (header, body) = match msg.split_once('\n') {
            Some((header, body)) => (header, Some(body)),
            None => (msg, None),
        };
        if header.len() > MAX_HEADER {
            return Err(WireError("the header is too long"));
        }
        let words: Vec<&str> = header.split(' ').collect();
        let request = match (words.as_slice(), body) {
            (["open", session, origin], None) => Request::Open {
                session: word(session, 32)?,
                origin: match *origin {
                    "origin=user" => Origin::User,
                    "origin=model" => Origin::Model,
                    _ => return Err(WireError("origin is user or model")),
                },
            },
            (["eval", seq], Some(entry)) => Request::Eval {
                seq: number(seq)?,
                entry: entry_text(entry)?,
            },
            (["replay", seq], Some(entry)) => Request::Replay {
                seq: number(seq)?,
                entry: entry_text(entry)?,
            },
            (["resume", id, status], Some(result)) => Request::Resume {
                id: number(id)?,
                ok: match *status {
                    "ok" => true,
                    "err" => false,
                    _ => return Err(WireError("resume is ok or err")),
                },
                result,
            },
            (["undo"], None) => Request::Undo,
            (["defs"], None) => Request::Defs,
            (["reset"], None) => Request::Reset,
            (["dis", name], None) => Request::Dis(ident(name)?),
            (["build", component, v], None) => Request::Build {
                component: word(component, 32)?,
                version: version(v)?,
            },
            _ => return Err(WireError("not a request")),
        };
        Ok(request)
    }

    /// The request as a message.
    pub fn text(&self) -> String {
        match self {
            Request::Open { session, origin } => {
                let origin = match origin {
                    Origin::User => "user",
                    Origin::Model => "model",
                };
                format!("open {session} origin={origin}")
            }
            Request::Eval { seq, entry } => format!("eval {seq}\n{entry}"),
            Request::Replay { seq, entry } => format!("replay {seq}\n{entry}"),
            Request::Resume { id, ok, result } => {
                let status = if *ok { "ok" } else { "err" };
                format!("resume {id} {status}\n{result}")
            }
            Request::Undo => "undo".into(),
            Request::Dis(name) => format!("dis {name}"),
            Request::Defs => "defs".into(),
            Request::Reset => "reset".into(),
            Request::Build { component, version } => format!("build {component} {version}"),
        }
    }
}

fn entry_text(entry: &str) -> Result<&str, WireError> {
    match entry.len() <= MAX_ENTRY {
        true => Ok(entry),
        false => Err(WireError("the entry is over 4 KiB")),
    }
}

/// A decimal number as written canonically: no sign, no leading zero.
fn number(text: &str) -> Result<u64, WireError> {
    let canonical = !text.is_empty()
        && text.bytes().all(|c| c.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    match canonical {
        true => text.parse().map_err(|_| WireError("a number is too big")),
        false => Err(WireError("not a number")),
    }
}

fn small(text: &str) -> Result<u32, WireError> {
    u32::try_from(number(text)?).map_err(|_| WireError("a number is too big"))
}

/// A lower-case name: letters, digits, `_` and `-`.
fn word(text: &str, max: usize) -> Result<&str, WireError> {
    let ok = !text.is_empty()
        && text.len() <= max
        && text
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'));
    ok.then_some(text).ok_or(WireError("not a name"))
}

/// A version's shape: digits and dots, at most 32 bytes. `cc build` checks
/// that it is `x.y.z`.
fn version(text: &str) -> Result<&str, WireError> {
    let ok = !text.is_empty()
        && text.len() <= 32
        && text.bytes().all(|c| c.is_ascii_digit() || c == b'.');
    ok.then_some(text).ok_or(WireError("not a version"))
}

/// A C identifier of at most 63 bytes.
fn ident(text: &str) -> Result<&str, WireError> {
    let ok = !text.is_empty()
        && text.len() <= 63
        && !text.as_bytes()[0].is_ascii_digit()
        && text.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
    ok.then_some(text).ok_or(WireError("not a C name"))
}

/// The type of a shown value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    I64,
    U64,
    F64,
    Ptr,
    Str,
}

impl ValueType {
    pub fn word(self) -> &'static str {
        match self {
            ValueType::I64 => "i64",
            ValueType::U64 => "u64",
            ValueType::F64 => "f64",
            ValueType::Ptr => "ptr",
            ValueType::Str => "str",
        }
    }

    fn parse(word: &str) -> Result<Self, WireError> {
        match word {
            "i64" => Ok(ValueType::I64),
            "u64" => Ok(ValueType::U64),
            "f64" => Ok(ValueType::F64),
            "ptr" => Ok(ValueType::Ptr),
            "str" => Ok(ValueType::Str),
            _ => Err(WireError("not a value type")),
        }
    }
}

fn kind(word: &str) -> Result<Kind, WireError> {
    use Kind::{Decl, Func, Include, Macro, Type, Typedef, Var};
    [Include, Macro, Type, Typedef, Var, Func, Decl]
        .into_iter()
        .find(|k| k.word() == word)
        .ok_or(WireError("not a definition kind"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record<'a> {
    Out(&'a [u8]),
    Def {
        kind: Kind,
        /// The table slot of a function.
        slot: Option<u32>,
        name: &'a str,
    },
    Retyped(&'a str),
    Value {
        ty: ValueType,
        text: &'a [u8],
    },
    /// An effect, with a number of its own, so an answer meant for another
    /// effect cannot be taken for this one.
    Need {
        id: u64,
        call: &'a [u8],
    },
    Done {
        compile_us: u64,
        link_us: u64,
        run_us: u64,
    },
    Err {
        line: u32,
        text: &'a [u8],
    },
    Trap {
        /// A word: `unreachable`, `memory`, `fuel`, `deadline`, `interrupted`…
        reason: &'a str,
        function: Option<&'a str>,
    },
    Undone(u64),
    Text(&'a [u8]),
    Ok,
}

impl Record<'_> {
    /// Whether its fields are ones [`parse_reply`] reads back.
    fn valid(&self) -> bool {
        match *self {
            Record::Def { name, .. } => def_name(name),
            Record::Retyped(name) => ident(name).is_ok(),
            Record::Trap { reason, function } => {
                word(reason, 32).is_ok() && function.is_none_or(|f| ident(f).is_ok())
            }
            _ => true,
        }
    }

    /// Whether it ends a reply.
    pub fn terminal(&self) -> bool {
        !matches!(
            self,
            Record::Out(_) | Record::Def { .. } | Record::Retyped(_) | Record::Value { .. }
        )
    }

    fn write(&self, out: &mut Vec<u8>) {
        let (header, block): (String, &[u8]) = match *self {
            Record::Out(bytes) => (format!("out {}", bytes.len()), bytes),
            Record::Def { kind, slot, name } => {
                let slot = slot.map_or(String::from("-"), |s| format!("{s}"));
                let header = format!("def {} {slot} {}", kind.word(), name.len());
                (header, name.as_bytes())
            }
            Record::Retyped(name) => (format!("retyped {name}"), &[]),
            Record::Value { ty, text } => (format!("value {} {}", ty.word(), text.len()), text),
            Record::Need { id, call } => (format!("need {id} {}", call.len()), call),
            Record::Done {
                compile_us,
                link_us,
                run_us,
            } => (format!("done {compile_us} {link_us} {run_us}"), &[]),
            Record::Err { line, text } => (format!("err {line} {}", text.len()), text),
            Record::Trap { reason, function } => {
                (format!("trap {reason} {}", function.unwrap_or("-")), &[])
            }
            Record::Undone(seq) => (format!("undone {seq}"), &[]),
            Record::Text(text) => (format!("text {}", text.len()), text),
            Record::Ok => ("ok".into(), &[]),
        };
        out.extend_from_slice(header.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(block);
    }
}

/// The record was not written: it did not fit, came after the terminal one, or
/// holds a field the reader would refuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused;

/// A reply, written record by record within `cap` bytes.
#[derive(Clone, Debug)]
pub struct Reply {
    bytes: Vec<u8>,
    cap: usize,
    records: usize,
    ended: bool,
}

impl Reply {
    pub fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::new(),
            cap,
            records: 0,
            ended: false,
        }
    }

    /// Bytes left.
    pub fn room(&self) -> usize {
        self.cap - self.bytes.len()
    }

    /// Adds `record`; nothing changes if it does not fit.
    pub fn push(&mut self, record: &Record) -> Result<(), Refused> {
        self.push_leaving(record, 0)
    }

    /// Adds `record` only if `keep` bytes are left after it, for the records
    /// still to come. A record that does not end the reply always leaves room
    /// for one that does: [`parse_reply`] reads at most [`MAX_RECORDS`].
    pub fn push_leaving(&mut self, record: &Record, keep: usize) -> Result<(), Refused> {
        let records = self.records + usize::from(!record.terminal());
        if self.ended || records >= MAX_RECORDS || !record.valid() {
            return Err(Refused);
        }
        let mut bytes = Vec::new();
        record.write(&mut bytes);
        if bytes.len().saturating_add(keep) > self.room() {
            return Err(Refused);
        }
        self.bytes.extend_from_slice(&bytes);
        self.records += 1;
        self.ended = record.terminal();
        Ok(())
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// The records of a reply; the last is its one terminal record.
pub fn parse_reply(msg: &[u8]) -> Result<Vec<Record<'_>>, WireError> {
    let mut records = Vec::new();
    let mut at = 0;
    while at < msg.len() {
        if records.len() == MAX_RECORDS {
            return Err(WireError("too many records"));
        }
        if records.last().is_some_and(Record::terminal) {
            return Err(WireError("bytes after the end of the reply"));
        }
        let (record, next) = parse_record(msg, at)?;
        records.push(record);
        at = next;
    }
    match records.last() {
        Some(last) if last.terminal() => Ok(records),
        _ => Err(WireError("the reply has no end")),
    }
}

/// The record at `at`, and where the next starts.
fn parse_record(msg: &[u8], at: usize) -> Result<(Record<'_>, usize), WireError> {
    let rest = &msg[at..];
    let nl = rest
        .iter()
        .take(MAX_HEADER + 1)
        .position(|&b| b == b'\n')
        .ok_or(WireError("a header has no end"))?;
    let header =
        core::str::from_utf8(&rest[..nl]).map_err(|_| WireError("a header is not text"))?;
    let words: Vec<&str> = header.split(' ').collect();
    let body = at + nl + 1;
    // A record carrying `n` bytes after its header.
    let block = |n: &str| -> Result<(&[u8], usize), WireError> {
        let n = usize::try_from(number(n)?).map_err(|_| WireError("a count is too big"))?;
        let end = body
            .checked_add(n)
            .filter(|&end| end <= msg.len())
            .ok_or(WireError("a count runs past the end"))?;
        Ok((&msg[body..end], end))
    };
    let (record, next) = match words.as_slice() {
        ["out", n] => {
            let (bytes, end) = block(n)?;
            (Record::Out(bytes), end)
        }
        ["def", k, slot, n] => {
            let (name, end) = block(n)?;
            let name = core::str::from_utf8(name)
                .ok()
                .filter(|s| def_name(s))
                .ok_or(WireError("not a definition name"))?;
            let slot = match *slot {
                "-" => None,
                slot => Some(small(slot)?),
            };
            let kind = kind(k)?;
            (Record::Def { kind, slot, name }, end)
        }
        ["retyped", name] => (Record::Retyped(ident(name)?), body),
        ["value", ty, n] => {
            let ty = ValueType::parse(ty)?;
            let (text, end) = block(n)?;
            (Record::Value { ty, text }, end)
        }
        ["need", id, n] => {
            let id = number(id)?;
            let (call, end) = block(n)?;
            (Record::Need { id, call }, end)
        }
        ["done", compile, link, run] => {
            let record = Record::Done {
                compile_us: number(compile)?,
                link_us: number(link)?,
                run_us: number(run)?,
            };
            (record, body)
        }
        ["err", line, n] => {
            let line = small(line)?;
            let (text, end) = block(n)?;
            (Record::Err { line, text }, end)
        }
        ["trap", reason, function] => {
            let function = match *function {
                "-" => None,
                name => Some(ident(name)?),
            };
            let reason = word(reason, 32)?;
            (Record::Trap { reason, function }, body)
        }
        ["undone", seq] => (Record::Undone(number(seq)?), body),
        ["text", n] => {
            let (text, end) = block(n)?;
            (Record::Text(text), end)
        }
        ["ok"] => (Record::Ok, body),
        _ => return Err(WireError("not a record")),
    };
    Ok((record, next))
}

/// A definition's name as replies give it: printable ASCII, at most
/// [`MAX_NAME`] bytes: `Sq`, `struct Point`, `#define N`, `<stdio.h>`.
fn def_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME && name.bytes().all(|c| (b' '..=b'~').contains(&c))
}
