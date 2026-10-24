//! `undo`, `undo N` and `tune undo` over the action log (ADR 0002 D7).
//!
//! - `undo` reverts the newest live entry, its changes newest first. The undo is
//!   an entry of its own, so `undo` again redoes.
//! - `undo N` reverts entry N, but only if no later live entry changed the same
//!   resource; otherwise it names those entries.
//! - `tune undo` is `undo` of the newest kept trial.
//! - Every inverse is compare-and-set: it runs only if the resource still reads
//!   what the change left. If it moved, the user is asked (`y` puts it back
//!   anyway). A switch the kernel refuses as too soon is tried once more.

use crate::actlog::{Change, Entry, Form, State};
use crate::undo::{self, Refused};
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

impl Shell {
    /// `undo` with nothing (the newest live entry) or an entry number.
    pub(crate) fn undo_command(&mut self, which: &str) -> Reply {
        let which = which.trim().trim_start_matches('#');
        if which.is_empty() {
            return match self.log.newest_live() {
                Some(id) => self.revert_entry(id, false),
                None => Reply::err("nothing to undo"),
            };
        }
        let Ok(id) = which.parse::<u64>() else {
            return Reply::err("usage: undo [entry number]");
        };
        self.undo_entry(id)
    }

    /// `tune undo`: the newest trial the kernel kept.
    pub(crate) fn tune_undo(&mut self) -> Reply {
        match self.log.newest_live_of(Form::Tune) {
            Some(id) => self.undo_entry(id),
            None => Reply::err("nothing to undo: no trial kept a policy"),
        }
    }

    /// Entry `id`, if it is live and nothing later changed what it changed.
    fn undo_entry(&mut self, id: u64) -> Reply {
        match self.log.get(id) {
            Some(entry) if entry.live() => {}
            Some(_) => return Reply::err(format!("#{id} has nothing to undo")),
            None => return Reply::err(format!("no logged entry #{id}")),
        }
        let later = self.log.later_touching(id);
        if !later.is_empty() {
            let names: Vec<String> = later.iter().map(|n| format!("#{n}")).collect();
            return Reply::err(format!(
                "{} changed the same thing since #{id}; undo {} first",
                names.join(", "),
                match later.len() {
                    1 => "it",
                    _ => "them",
                }
            ));
        }
        self.revert_entry(id, false)
    }

    /// The answer to `undo`'s question about a value that moved since.
    pub(crate) fn undo_answer(&mut self, id: u64, yes: bool) -> Reply {
        match yes {
            true => self.revert_entry(id, true),
            false => Reply::ok("Not undone; `undo` asks again"),
        }
    }

    /// Puts entry `id`'s changes back, newest first, and logs the undo as an entry
    /// whose changes put them back again.
    pub(crate) fn revert_entry(&mut self, id: u64, anyway: bool) -> Reply {
        let Some(entry) = self.log.get(id).cloned() else {
            return Reply::err("nothing to undo");
        };
        let mut said = Vec::new();
        let mut redo: Vec<Change> = Vec::new();
        let mut left = entry.changes.clone();
        let mut stopped: Option<Reply> = None;
        while let Some(change) = left.pop() {
            let Some(inverse) = change.inverse else {
                left.push(change);
                break;
            };
            match undo::revert(self.host.as_ref(), inverse, anyway) {
                Ok(text) => {
                    said.push(text);
                    redo.push(Change {
                        before: change.after.clone(),
                        after: change.before.clone(),
                        inverse: Some(undo::reverse(inverse)),
                        ..change
                    });
                }
                Err(Refused::Moved(now)) => {
                    self.undo_asking = Some(id);
                    self.host
                        .audit(&format!("undo #{id} asks: changed since (now {now})"));
                    let mut text = undone(&said);
                    text.push_str(&format!(
                        "The {} changed since (now {now}); put {} back anyway? y/n",
                        change.res,
                        undo::target(inverse)
                    ));
                    left.push(change);
                    stopped = Some(Reply::ok(text));
                    break;
                }
                Err(Refused::Failed(why)) => {
                    self.host.audit(&format!("undo #{id} failed: {why}"));
                    left.push(change);
                    stopped = Some(Reply::err(format!(
                        "{}{why}; try undo again",
                        undone(&said)
                    )));
                    break;
                }
            }
        }
        if !redo.is_empty() {
            self.log_undo(&entry, left, redo);
        }
        stopped.unwrap_or_else(|| Reply::ok(format!("Undone: {}", said.join("; "))))
    }

    /// The undo of `entry` as an entry: what it put back, and what of `entry` is
    /// still to undo.
    fn log_undo(&mut self, entry: &Entry, left: Vec<Change>, redo: Vec<Change>) {
        let id = self.next_id;
        self.next_id += 1;
        if let Some(original) = self.log.get_mut(entry.id) {
            original.changes = left;
            if original.changes.is_empty() {
                original.state = State::Undone;
            }
        }
        self.host.audit(&format!("undo #{id}: undid #{}", entry.id));
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: Form::Undo,
            of: Some(entry.id),
            tier: None,
            consent: String::from("typed"),
            state: State::Done,
            request: String::from("undo"),
            script: String::new(),
            outcome: String::from("done"),
            changes: redo,
        });
    }

    /// Logs a trial the kernel kept, if one was, so `undo` can put it back.
    pub(crate) fn log_kept_trial(&mut self) {
        let Some(kept) = self.tuner.take_kept() else {
            return;
        };
        let (before, after) = (kept.before, kept.after);
        let change = Change {
            action: String::from("tune"),
            owner: String::from("kernel"),
            res: String::from("policy"),
            before: undo::spec_name(before.0, before.1),
            after: undo::spec_name(after.0, after.1),
            inverse: Some(undo::Inverse::Spec {
                policy: before.0,
                preset: before.1,
                after,
            }),
        };
        self.log.push(Entry {
            id: kept.id,
            t_ns: self.host.now_ns(),
            form: Form::Tune,
            of: None,
            tier: None,
            consent: String::from("yes"),
            state: State::Done,
            request: format!("tune {}", kept.target),
            script: String::new(),
            outcome: format!("kept {}", change.after),
            changes: alloc::vec![change],
        });
    }
}

/// `Undone: …` and a newline when something was put back before a stop.
fn undone(said: &[String]) -> String {
    match said.is_empty() {
        true => String::new(),
        false => format!("Undone: {}\n", said.join("; ")),
    }
}
