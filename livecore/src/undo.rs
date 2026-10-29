//! Checkpoint undo (ADR 0003 section 4): the host keeps the state before and
//! after each of the last [`KEPT`] calls that changed it, in its own memory. An
//! undo restores the state from before the call if the state still equals the
//! one after it (ADR 0002's compare-and-set), and otherwise answers `changed`, so
//! the agent asks before sending it again with `force`. An undo is itself kept,
//! so undoing it redoes. The history is the active version's: a swap that commits
//! and `ctl reset` clear it, and a host restart loses it.
//!
//! ```text
//! kai-undo 1
//! id 7
//! force            (optional)
//! ```
//!
//! answered `kai-reply 1` with `undone 7` and the new entry's `undo <id>`,
//! `changed 7`, or `refused <why>`.

use crate::call::{REPLY_HEADER, format_refusal};
use crate::host::{Env, Host, Mode, Report, Stop, Version, text};
use crate::txn::Storage;
use crate::{Fault, Instance, Reason};
use alloc::collections::VecDeque;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::live::kind;

pub const UNDO_HEADER: &str = "kai-undo 1";
/// Calls whose undo is kept.
pub const KEPT: usize = 16;

/// The app's state and storage as the kernel holds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Saved {
    pub state: Vec<u8>,
    pub storage: Storage,
}

struct Entry {
    id: u64,
    before: Rc<Saved>,
    after: Rc<Saved>,
}

#[derive(Default)]
pub(crate) struct History {
    entries: VecDeque<Entry>,
    next_id: u64,
}

impl History {
    /// Keeps a change from `before` to `after`; its id.
    pub fn record(&mut self, before: Saved, after: Saved) -> u64 {
        // The state before a call is usually the one after the last: share it.
        let before = match self.entries.back() {
            Some(last) if *last.after == before => Rc::clone(&last.after),
            _ => Rc::new(before),
        };
        if self.entries.len() == KEPT {
            self.entries.pop_front();
        }
        self.next_id += 1;
        self.entries.push_back(Entry {
            id: self.next_id,
            before,
            after: Rc::new(after),
        });
        self.next_id
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    fn find(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }
}

/// A parsed `kai-undo 1`: the entry and whether to undo even if it changed.
fn parse(request: &[u8]) -> Result<(u64, bool), String> {
    let text = core::str::from_utf8(request).map_err(|_| String::from("the undo is not text"))?;
    let mut lines = text.lines().filter(|line| !line.is_empty());
    if lines.next() != Some(UNDO_HEADER) {
        return Err(String::from("not a kai-undo 1 request"));
    }
    let id = lines
        .next()
        .and_then(|line| line.strip_prefix("id "))
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| String::from("expected id <number>"))?;
    let force = match lines.next() {
        None => false,
        Some("force") => true,
        Some(other) => return Err(format!("unexpected line {other:?}")),
    };
    if lines.next().is_some() {
        return Err(String::from("unexpected lines after the undo"));
    }
    Ok((id, force))
}

impl<E: Env> Host<E> {
    /// Answers a `kai-undo 1` request.
    pub(crate) fn undo(
        &mut self,
        mode: Mode<E::Instance>,
        request: &[u8],
    ) -> Result<(Mode<E::Instance>, Vec<u8>), crate::host::Exit> {
        let (id, force) = match parse(request) {
            Ok(parsed) => parsed,
            Err(why) => return Ok((mode, text(format_refusal(&why)))),
        };
        let Mode::Serving(mut version) = mode else {
            let why = "an undo waits until no swap is on probation and the app is enabled";
            return Ok((mode, text(format_refusal(why))));
        };
        let core = &mut self.core;
        let Some(entry) = core.history.find(id) else {
            let why = format!("no undo {id}: only the last {KEPT} changes are kept");
            return Ok((Mode::Serving(version), text(format_refusal(&why))));
        };
        let now = Saved {
            state: core.checkpoint.clone(),
            storage: core.storage.clone(),
        };
        if now != *entry.after && !force {
            return Ok((
                Mode::Serving(version),
                text(format!("{REPLY_HEADER}\nchanged {id}\n")),
            ));
        }
        let before = Rc::clone(&entry.before);
        match restore(core, &mut version, &before) {
            Ok(()) => {}
            Err(Stop::Exit(exit)) => return Err(exit),
            Err(Stop::Fault(fault)) => {
                let why = format!(
                    "undo {id} failed: {}: {}",
                    fault.reason.name(),
                    fault.detail
                );
                return Ok((Mode::Serving(version), text(format_refusal(&why))));
            }
        }
        let redo = core.history.record(now, (*before).clone());
        let reply = format!("{REPLY_HEADER}\nundone {id}\nundo {redo}\n");
        Ok((Mode::Serving(version), text(reply)))
    }
}

/// Puts `saved` into a fresh instance of `version` and checkpoints it.
fn restore<E: Env>(
    core: &mut crate::host::Core<E>,
    version: &mut Version<E::Instance>,
    saved: &Saved,
) -> Result<(), Stop> {
    let mut instance = core.env.load(&version.artifact, false)?;
    instance.view(&saved.storage);
    match instance.import(&saved.state, version.schema)? {
        0 => {}
        code => {
            return Err(Stop::Fault(Fault::new(
                Reason::Migrate,
                format!("it refused the saved state ({code})"),
            )));
        }
    }
    core.report(&Report {
        schema: u64::from(version.schema),
        blob: &saved.state,
        storage: Some(&saved.storage),
        ..Report::new(kind::CHECKPOINT)
    })?;
    version.instance = instance;
    core.checkpoint = saved.state.clone();
    core.storage = saved.storage.clone();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn saved(n: u8) -> Saved {
        Saved {
            state: vec![0x82, 1, n],
            storage: Storage::new(),
        }
    }

    #[test]
    fn the_history_keeps_the_last_changes_and_shares_states() {
        let mut history = History::default();
        let first = history.record(saved(0), saved(1));
        let second = history.record(saved(1), saved(2));
        assert_eq!((first, second), (1, 2));
        let entry = history.find(2).unwrap();
        assert!(Rc::ptr_eq(&entry.before, &history.find(1).unwrap().after));
        for n in 2..40 {
            history.record(saved(n), saved(n + 1));
        }
        assert!(history.find(1).is_none());
        assert_eq!(history.entries.len(), KEPT);
        history.clear();
        assert!(history.find(40).is_none());
    }

    #[test]
    fn undo_requests_parse() {
        assert_eq!(parse(b"kai-undo 1\nid 7\n"), Ok((7, false)));
        assert_eq!(parse(b"kai-undo 1\nid 7\nforce\n"), Ok((7, true)));
        for bad in [
            &b"kai-undo 2\nid 7\n"[..],
            b"kai-undo 1\n",
            b"kai-undo 1\nid x\n",
            b"kai-undo 1\nid 7\nplease\n",
            b"kai-undo 1\nid 7\nforce\nmore\n",
            b"\xff",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
