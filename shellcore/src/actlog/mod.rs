//! The action log, `actlog 1` (ADR 0002 D7): one entry per request, a saga of the
//! changes it made, each with the value it replaced and how to put it back. It
//! replaces the script log, the one-deep script undo slot and `tune undo`'s own
//! slot. 64 records in memory in P1; from P2 the same text goes to `/system/log`.
//!
//! `undo` reverts the newest live entry, its changes newest first. An undo is
//! itself an entry (a `U` record), as in Emacs, so `undo` again redoes. `undo N`
//! reverts entry N only if no later live entry touched the same resource. System
//! events (`S` records) are logged too and never undone.

mod text;

pub use text::{MAX_TEXT, ParseError, parse, parse_setting_aside};

use crate::tiers::Tier;
use crate::undo::Inverse;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// Records kept in memory.
pub const KEEP: usize = 64;
/// Longest request shown in `log`'s list.
const REQUEST_CHARS: usize = 60;

/// `text` as one log record carries it: a longer one would make the whole
/// log on disk unreadable at the next start ([`MAX_TEXT`]).
pub fn clip(text: &str) -> String {
    crate::show::truncate(text, MAX_TEXT - " ...".len())
}

/// What made an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// One tagged call from the model.
    Call,
    /// A script, from `script` or today's script request.
    Script,
    /// An answer in words, a question back, or nothing to run.
    Words,
    /// A kept `tune` trial.
    Tune,
    /// An undo of entry `of`.
    Undo,
    /// A standing instruction's firing (P3 task 11).
    Standing,
}

impl Form {
    pub fn word(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::Script => "script",
            Self::Words => "words",
            Self::Tune => "tune",
            Self::Undo => "undo",
            Self::Standing => "standing",
        }
    }
}

/// Where an entry stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// It ran, and its changes stand.
    Done,
    /// It asked, and waits for the answer.
    Asked,
    /// It was refused, declined, expired or failed, and changed nothing.
    Nothing,
    /// A later undo put its changes back.
    Undone,
    /// It failed after an app took a call and did not answer: that call may
    /// have run, so it is neither done nor nothing (audit K #19).
    Unknown,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Asked => "asked",
            Self::Nothing => "nothing",
            Self::Undone => "undone",
            Self::Unknown => "unknown",
        }
    }
}

/// One change an entry made (a `D` record).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The registry name that made it: `set_policy`.
    pub action: String,
    /// Who executes it and its inverse: `kernel`.
    pub owner: String,
    /// The resource it changed: `policy`. Two changes to one resource overlap.
    pub res: String,
    pub before: String,
    pub after: String,
    /// How to put it back, while `after` is still what is there.
    pub inverse: Option<Inverse>,
}

/// The start of an installed app's resource, `app <service>`: two changes to
/// one app overlap, whichever rows made them.
pub const APP_RES: &str = "app ";

/// Whether `a` and `b` name one resource. The log on disk keeps a resource
/// as one word, `_` for each space, so one read back after a restart is
/// compared in that form (bug-hunt-3 H3-24).
pub fn same_res(a: &str, b: &str) -> bool {
    a == b || text::word(a) == text::word(b)
}

impl Change {
    /// `policy round-robin (was interactive)`, as the result line says it; with
    /// no value before, no "(was …)". An app's change says itself in `after`
    /// (`add a note in Notes`): its `app <service>` resource is for overlap.
    pub fn what(&self) -> String {
        let said = match self.res.starts_with(APP_RES) {
            true => self.after.clone(),
            false => format!("{} {}", self.res, self.after),
        };
        match self.before.is_empty() {
            true => said,
            false => format!("{said} (was {})", self.before),
        }
    }

    /// Whether this change and `other` changed one resource ([`same_res`]).
    pub fn overlaps(&self, other: &Change) -> bool {
        same_res(&self.res, &other.res)
    }
}

/// One entry: a request and what became of it, or an undo (`of`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub t_ns: u64,
    pub form: Form,
    /// The entry an undo reverted.
    pub of: Option<u64>,
    pub tier: Option<Tier>,
    /// The `pack_id` of the pack the model was given (ADR 0010 §7), for an entry
    /// a model turn made; `None` for one the user typed.
    pub pack: Option<u64>,
    /// Where consent came from: `asked`, `taken-as`, `yes`, `typed`, `-`.
    pub consent: String,
    pub state: State,
    /// The user's line, as typed or pasted.
    pub request: String,
    /// The script or call that ran, for `log <id>`.
    pub script: String,
    /// What became of it, in code's words: `ran`, `refused: …`.
    pub outcome: String,
    pub changes: Vec<Change>,
    /// Whose entry it is (ADR 0013 §1, `actlog 2`): `kai`, the person's shell,
    /// unless an agent's [`agentid::Stamp`] made it.
    pub agent: agentid::Name,
}

impl Entry {
    /// The entry as agent `stamp` made it: the name comes from the caller the
    /// kernel's badge named, never from the request.
    pub fn stamped(self, stamp: &agentid::Stamp) -> Self {
        Self {
            agent: stamp.name().clone(),
            ..self
        }
    }

    /// Whether `undo` may revert it: it ran, made changes, and every one has an
    /// inverse this shell can read.
    pub fn live(&self) -> bool {
        self.state == State::Done && self.changes_undoable()
    }

    /// Whether `undo <id>` may put its known changes back: a live entry, or an
    /// unknown outcome whose changes that are known all have an inverse. A
    /// bare `undo` never picks the second (#1200's review): the call whose
    /// outcome is unknown is not among them.
    pub fn undoable_by_id(&self) -> bool {
        matches!(self.state, State::Done | State::Unknown) && self.changes_undoable()
    }

    fn changes_undoable(&self) -> bool {
        !self.changes.is_empty()
            && self
                .changes
                .iter()
                .all(|c| c.inverse.is_some_and(|i| i.is_read()))
    }
}

/// Something no request caused (an `S` record): a driver reset, a trial's end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct System {
    pub id: u64,
    pub t_ns: u64,
    pub owner: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Entry(Entry),
    System(System),
}

#[derive(Default)]
pub struct ActLog {
    records: VecDeque<Record>,
}

impl ActLog {
    /// A log read back from `/system/log` (P2 task 12): its newest [`KEEP`]
    /// records.
    pub fn from_records(records: Vec<Record>) -> Self {
        let skip = records.len().saturating_sub(KEEP);
        Self {
            records: records.into_iter().skip(skip).collect(),
        }
    }

    /// The highest entry or record number in the log, 0 for none.
    pub fn last_id(&self) -> u64 {
        self.records
            .iter()
            .map(|r| match r {
                Record::Entry(e) => e.id,
                Record::System(s) => s.id,
            })
            .max()
            .unwrap_or(0)
    }

    pub fn push(&mut self, entry: Entry) {
        self.keep(Record::Entry(entry));
    }

    pub fn system(&mut self, system: System) {
        self.keep(Record::System(system));
    }

    fn keep(&mut self, record: Record) {
        // An id names one record: `undo N` and `log N` find the first that has it, and
        // a log that holds one twice is refused when it is read back (G24).
        let id = |r: &Record| match r {
            Record::Entry(e) => e.id,
            Record::System(s) => s.id,
        };
        debug_assert!(
            !self.records.iter().any(|r| id(r) == id(&record)),
            "#{} is logged twice",
            id(&record)
        );
        if self.records.len() == KEEP {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }

    pub(crate) fn entries(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.records.iter().filter_map(|r| match r {
            Record::Entry(e) => Some(e),
            Record::System(_) => None,
        })
    }

    pub(crate) fn entries_mut(&mut self) -> impl Iterator<Item = &mut Entry> {
        self.records.iter_mut().filter_map(|r| match r {
            Record::Entry(e) => Some(e),
            Record::System(_) => None,
        })
    }

    /// Takes the entries numbered in `ids` out of the log, oldest first,
    /// leaving every other record: a `c>` entry's own calls, to be logged as
    /// one.
    pub fn take_entries_of(&mut self, ids: &[u64]) -> Vec<Entry> {
        let mut taken = Vec::new();
        self.records.retain(|record| match record {
            Record::Entry(e) if ids.contains(&e.id) => {
                taken.push(e.clone());
                false
            }
            _ => true,
        });
        taken
    }

    pub fn get(&self, id: u64) -> Option<&Entry> {
        self.entries().find(|e| e.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Entry> {
        self.records.iter_mut().find_map(|r| match r {
            Record::Entry(e) if e.id == id => Some(e),
            _ => None,
        })
    }

    /// The newest entry `undo` would revert.
    pub fn newest_live(&self) -> Option<u64> {
        self.entries().rev().find(|e| e.live()).map(|e| e.id)
    }

    /// The entry a bare `undo` reverts: the newest live entry, unless that
    /// is the undo of an entry an earlier `undo` stopped partway (still live,
    /// with changes left). Then that entry, so `undo` goes on with it rather
    /// than redoing what was just put back (bug-hunt-3 H3-22).
    pub fn undo_target(&self) -> Option<u64> {
        let newest = self.newest_live()?;
        let partway = self
            .get(newest)
            .and_then(|e| e.of)
            .filter(|&of| self.get(of).is_some_and(Entry::live));
        Some(partway.unwrap_or(newest))
    }

    /// The newest live entry of `form`.
    pub fn newest_live_of(&self, form: Form) -> Option<u64> {
        self.entries()
            .rev()
            .find(|e| e.live() && e.form == form)
            .map(|e| e.id)
    }

    /// Live entries after `id` that changed a resource entry `id` changed: `undo
    /// id` would overwrite them. An undo of a change made after `id` cancels that
    /// change, so it does not count, and neither does a held send: cancelling
    /// one never overwrites another to the same host.
    pub fn later_touching(&self, id: u64) -> Vec<u64> {
        let Some(entry) = self.get(id) else {
            return Vec::new();
        };
        let cancels_later = |e: &Entry| {
            e.of.is_some_and(|of| of > id && self.get(of).is_some_and(|t| t.state == State::Undone))
        };
        self.entries()
            .filter(|e| e.id > id && e.live() && !cancels_later(e))
            .filter(|e| {
                let sent = |c: &&Change| c.action != crate::outbox::ACTION;
                e.changes
                    .iter()
                    .filter(sent)
                    .any(|c| entry.changes.iter().filter(sent).any(|d| d.overlaps(c)))
            })
            .map(|e| e.id)
            .collect()
    }

    /// One line per entry, oldest first: `#3 switch to round robin: ran`.
    pub fn list(&self) -> String {
        let mut text = String::new();
        for entry in self.entries() {
            let request = match entry.of {
                Some(of) => format!("undo of #{of}"),
                None => crate::show::truncate(&entry.request, REQUEST_CHARS),
            };
            // An agent's entry names it; the person's own stay as they were.
            let by = match entry.agent.is_kai() {
                true => String::new(),
                false => format!("[{}] ", entry.agent.as_str()),
            };
            let _ = writeln!(text, "#{} {by}{request}: {}", entry.id, entry.outcome);
        }
        match text.is_empty() {
            true => String::from("nothing logged yet"),
            false => String::from(text.trim_end()),
        }
    }

    /// The script entry `id` ran, with its id.
    pub fn script(&self, id: u64) -> Option<String> {
        self.get(id)
            .filter(|e| !e.script.is_empty())
            .map(|e| format!("script {}:\n{}", e.id, e.script))
    }

    /// The log as `actlog 1` text, or `actlog 2` once an entry is an agent's.
    pub fn text(&self) -> String {
        text::write(self.records.iter())
    }
}

#[cfg(test)]
mod tests;
