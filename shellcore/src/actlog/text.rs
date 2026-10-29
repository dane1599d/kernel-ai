//! `actlog 1` as text (ADR 0002 D7): one record per line, free text
//! length-prefixed on the lines after it, so no request or script can forge a
//! record.
//!
//! ```text
//! actlog 1
//! E 17 t=1792618800123456789 reg=1 pack=0123456789abcdef form=call tier=run consent=asked state=done
//! R 21
//! switch to round robin
//! O 3
//! ran
//! D 17.1 set_policy owner=kernel res=policy before=interactive after=round-robin inv=spec:1:0:2:0
//! U 18 of=17 t=1792618860000000000 state=done
//! S 19 t=1792618860000000000 owner=kernel 33
//! nvme0 reset after 5 s timeout, ok
//! ```
//!
//! `R` is the request and `O` the outcome; a script's text (`C`) is kept in
//! memory for `log <id>` and written too. `D` values are single words. The
//! reader takes back what the writer wrote, each change's inverse with it
//! (`inv=spec:…`, `inv=fs:12`), so `undo` works on a log read from disk (P2
//! task 12).

use super::{Change, Entry, Form, Record, State, System};
use crate::tiers::Tier;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;

pub const HEADER: &str = "actlog 1";
/// Longest free text one record carries.
pub const MAX_TEXT: usize = 8 * 1024;
/// Most records a text may hold.
pub const MAX_RECORDS: usize = 4 * super::KEEP;

pub(super) fn write<'a>(records: impl Iterator<Item = &'a Record>) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for record in records {
        match record {
            Record::Entry(e) => entry(&mut out, e),
            Record::System(s) => {
                let _ = writeln!(
                    out,
                    "S {} t={} owner={} {}",
                    s.id,
                    s.t_ns,
                    word(&s.owner),
                    s.text.len()
                );
                let _ = writeln!(out, "{}", s.text);
            }
        }
    }
    out
}

fn entry(out: &mut String, e: &Entry) {
    match e.of {
        Some(of) => {
            let _ = writeln!(
                out,
                "U {} of={of} t={} state={}",
                e.id,
                e.t_ns,
                e.state.word()
            );
        }
        None => {
            let tier = e.tier.map_or("-", Tier::name);
            let _ = writeln!(
                out,
                "E {} t={} reg={} pack={} form={} tier={tier} consent={} state={}",
                e.id,
                e.t_ns,
                agentabi::REGISTRY_VERSION,
                e.pack
                    .map_or_else(|| String::from("-"), |pack| alloc::format!("{pack:016x}")),
                e.form.word(),
                word(&e.consent),
                e.state.word()
            );
        }
    }
    for (tag, text) in [("R", &e.request), ("O", &e.outcome), ("C", &e.script)] {
        if !text.is_empty() {
            let _ = writeln!(out, "{tag} {}\n{text}", text.len());
        }
    }
    for (n, c) in e.changes.iter().enumerate() {
        let inv = crate::undo::inverse_word(c.inverse);
        let _ = writeln!(
            out,
            "D {}.{} {} owner={} res={} before={} after={} inv={inv}",
            e.id,
            n + 1,
            word(&c.action),
            word(&c.owner),
            word(&c.res),
            word(&c.before),
            word(&c.after)
        );
    }
}

/// A value as one word: spaces and `=` would split a field, so they are `_`.
fn word(text: &str) -> String {
    match text.is_empty() {
        true => String::from("-"),
        false => text.replace([' ', '=', '\n'], "_"),
    }
}

/// Why a text is not `actlog 1`, and the line it went wrong on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub why: &'static str,
}

/// Reads `actlog 1` text back into records.
pub fn parse(text: &str) -> Result<Vec<Record>, ParseError> {
    let mut reader = Reader {
        rest: text,
        line: 0,
    };
    if reader.line_text()? != HEADER {
        return reader.fail("not actlog 1");
    }
    let mut records: Vec<Record> = Vec::new();
    while !reader.rest.is_empty() {
        if records.len() == MAX_RECORDS {
            return reader.fail("too many records");
        }
        let line = reader.line_text()?;
        let (tag, body) = line
            .split_once(' ')
            .ok_or_else(|| reader.error("a record has no fields"))?;
        match tag {
            "E" | "U" => records.push(Record::Entry(reader.entry(tag, body)?)),
            "S" => records.push(Record::System(reader.system(body)?)),
            "R" | "O" | "C" => {
                let len = length(body).ok_or_else(|| reader.error("a bad text length"))?;
                let text = reader.text(len)?;
                let Some(Record::Entry(e)) = records.last_mut() else {
                    return reader.fail("text before any entry");
                };
                let slot = match tag {
                    "R" => &mut e.request,
                    "O" => &mut e.outcome,
                    _ => &mut e.script,
                };
                if !slot.is_empty() {
                    return reader.fail("a text appears twice in one entry");
                }
                *slot = text;
            }
            "D" => {
                let change = reader.change(body)?;
                let Some(Record::Entry(e)) = records.last_mut() else {
                    return reader.fail("a change before any entry");
                };
                if change.0 != e.id || change.1 != e.changes.len() + 1 {
                    return reader.fail("a change out of order");
                }
                e.changes.push(change.2);
            }
            _ => return reader.fail("an unknown record"),
        }
    }
    Ok(records)
}

fn length(text: &str) -> Option<usize> {
    let n: usize = text.parse().ok()?;
    (n <= MAX_TEXT && text == n.to_string()).then_some(n)
}

fn number(text: &str) -> Option<u64> {
    let n: u64 = text.parse().ok()?;
    (text == n.to_string()).then_some(n)
}

struct Reader<'a> {
    rest: &'a str,
    line: usize,
}

impl<'a> Reader<'a> {
    fn error(&self, why: &'static str) -> ParseError {
        ParseError {
            line: self.line,
            why,
        }
    }

    fn fail<T>(&self, why: &'static str) -> Result<T, ParseError> {
        Err(self.error(why))
    }

    fn line_text(&mut self) -> Result<&'a str, ParseError> {
        let (line, rest) = self
            .rest
            .split_once('\n')
            .ok_or_else(|| self.error("a line does not end"))?;
        self.line += 1;
        self.rest = rest;
        Ok(line)
    }

    /// `len` bytes of free text, then its newline.
    fn text(&mut self, len: usize) -> Result<String, ParseError> {
        let text = self
            .rest
            .get(..len)
            .ok_or_else(|| self.error("a text runs past the end"))?;
        let rest = self.rest[len..]
            .strip_prefix('\n')
            .ok_or_else(|| self.error("a text is longer than it says"))?;
        self.line += 1 + text.matches('\n').count();
        self.rest = rest;
        Ok(text.to_string())
    }

    fn fields(&self, body: &'a str, keys: &[&str]) -> Result<Vec<&'a str>, ParseError> {
        let mut words = body.split(' ');
        let mut values = Vec::new();
        for key in keys {
            let word = words
                .next()
                .ok_or_else(|| self.error("a field is missing"))?;
            let value = match key.is_empty() {
                true => word,
                false => word
                    .strip_prefix(key)
                    .and_then(|w| w.strip_prefix('='))
                    .ok_or_else(|| self.error("a field is out of order"))?,
            };
            if value.is_empty() {
                return self.fail("a field is empty");
            }
            values.push(value);
        }
        match words.next() {
            Some(_) => self.fail("a record has extra fields"),
            None => Ok(values),
        }
    }

    fn entry(&self, tag: &str, body: &'a str) -> Result<Entry, ParseError> {
        let bad = |why| self.error(why);
        let mut pack = None;
        let (id, of, t, form, tier, consent, state) = match tag {
            "E" => {
                let v = self.fields(
                    body,
                    &["", "t", "reg", "pack", "form", "tier", "consent", "state"],
                )?;
                if v[2] != "1" {
                    return self.fail("an entry from another registry version");
                }
                pack = match v[3] {
                    "-" => None,
                    hex if hex.len() == 16 => {
                        Some(u64::from_str_radix(hex, 16).map_err(|_| bad("a bad pack_id"))?)
                    }
                    _ => return self.fail("a bad pack_id"),
                };
                let v = [v[0], v[1], v[2], v[4], v[5], v[6], v[7]];
                (
                    v[0],
                    None,
                    v[1],
                    form_of(v[3]).ok_or(bad("an unknown form"))?,
                    v[4],
                    v[5],
                    v[6],
                )
            }
            _ => {
                let v = self.fields(body, &["", "of", "t", "state"])?;
                let of = number(v[1]).ok_or(bad("a bad entry number"))?;
                (v[0], Some(of), v[2], Form::Undo, "-", "-", v[3])
            }
        };
        Ok(Entry {
            id: number(id).ok_or(bad("a bad entry number"))?,
            t_ns: number(t).ok_or(bad("a bad time"))?,
            form,
            of,
            pack,
            tier: match tier {
                "-" => None,
                t => Some(tier_of(t).ok_or(bad("an unknown tier"))?),
            },
            consent: consent.to_string(),
            state: state_of(state).ok_or(bad("an unknown state"))?,
            request: String::new(),
            script: String::new(),
            outcome: String::new(),
            changes: Vec::new(),
        })
    }

    fn system(&mut self, body: &'a str) -> Result<System, ParseError> {
        let v = self.fields(body, &["", "t", "owner", ""])?;
        let bad = |why| self.error(why);
        let (id, t_ns) = (
            number(v[0]).ok_or(bad("a bad record number"))?,
            number(v[1]).ok_or(bad("a bad time"))?,
        );
        let owner = v[2].to_string();
        let len = length(v[3]).ok_or(bad("a bad text length"))?;
        Ok(System {
            id,
            t_ns,
            owner,
            text: self.text(len)?,
        })
    }

    /// `D <entry>.<n> <action> owner= res= before= after= inv=`.
    fn change(&self, body: &'a str) -> Result<(u64, usize, Change), ParseError> {
        let v = self.fields(body, &["", "", "owner", "res", "before", "after", "inv"])?;
        let bad = |why| self.error(why);
        let (entry, n) = v[0].split_once('.').ok_or(bad("a bad change number"))?;
        let entry = number(entry).ok_or(bad("a bad change number"))?;
        let n = number(n).ok_or(bad("a bad change number"))? as usize;
        let inverse = crate::undo::parse_inverse(v[6]).ok_or(bad("an unknown inverse"))?;
        Ok((
            entry,
            n,
            Change {
                action: v[1].to_string(),
                owner: v[2].to_string(),
                res: v[3].to_string(),
                before: v[4].to_string(),
                after: v[5].to_string(),
                inverse,
            },
        ))
    }
}

fn form_of(word: &str) -> Option<Form> {
    [
        Form::Call,
        Form::Script,
        Form::Words,
        Form::Tune,
        Form::Standing,
    ]
    .into_iter()
    .find(|f| f.word() == word)
}

fn state_of(word: &str) -> Option<State> {
    [State::Done, State::Asked, State::Nothing, State::Undone]
        .into_iter()
        .find(|s| s.word() == word)
}

fn tier_of(word: &str) -> Option<Tier> {
    [Tier::Run, Tier::Ask, Tier::Never]
        .into_iter()
        .find(|t| t.name() == word)
}

/// Kept for errors that name a line: `line 4: a field is missing`.
impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "line {}: {}", self.line, self.why)
    }
}
