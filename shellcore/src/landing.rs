//! What a line whose file changes did not land says (#1306): its reply as
//! it was, its output, its other changes and its own error kept, less the
//! "Done" words for the file changes that did not land, and a note that
//! they did not. The panel and the desk show the same, never "Done" for
//! them.

use crate::{Reply, Shell, ledger};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem::take;

/// A line's file changes that did not land, until its reply says so.
#[derive(Default)]
pub(crate) struct Landing {
    /// What the last log write said of the entries it took them from.
    notes: Vec<String>,
    /// What those changes said in the line's "Done" line.
    lost: Vec<String>,
    /// The last line's result for the panel and the desk.
    line: Option<String>,
}

impl Shell {
    /// The log write found `txn` did not land: its file changes go from the
    /// entries, the audit says so, and the line's reply will; the error.
    pub(crate) fn not_landed(&mut self, txn: u64, why: &str) -> String {
        for (id, whats) in ledger::take_not_landed(&mut self.log, txn, why) {
            let note =
                format!("#{id}: its file changes did not land ({why}); nothing of them to undo");
            self.host.audit(&note);
            self.landing.notes.push(note);
            self.landing.lost.extend(whats);
        }
        format!("the files did not land: {why}")
    }

    /// A line begins: a loss another write found (a send's) is a notice.
    pub(crate) fn landing_begins(&mut self) {
        let earlier = take(&mut self.landing.notes);
        self.notices.extend(earlier);
        self.landing.lost.clear();
        self.landing.line = None;
    }

    /// The line's reply, saying what of its file changes did not land.
    pub(crate) fn say_not_landed(&mut self, reply: Reply) -> Reply {
        let notes = take(&mut self.landing.notes);
        let lost = take(&mut self.landing.lost);
        if notes.is_empty() {
            return reply;
        }
        let note = notes.join("\n");
        let done = self.done_line.take();
        let rest = done.as_deref().and_then(|line| without_lost(line, &lost));
        let mut lines: Vec<String> = Vec::new();
        for line in reply.text.lines() {
            match done.as_deref() == Some(line) {
                true => lines.extend(rest.clone()),
                false => lines.push(String::from(line)),
            }
        }
        // A question still waiting stays last.
        match reply.confirm {
            Some(_) => lines.insert(0, note.clone()),
            None => lines.push(note.clone()),
        }
        self.landing.line = Some(match &rest {
            Some(rest) => format!("{}; {note}", crate::panel::without_undo(rest)),
            None => note,
        });
        self.done_line = rest;
        Reply {
            ok: false,
            text: lines.join("\n"),
            ..reply
        }
    }

    /// A change saved outside a line (the editor's): `Err` with what the log
    /// write said when its file changes did not land, so it is never told
    /// it saved. Run [`Shell::landing_begins`] before the change.
    pub(crate) fn landed(&mut self) -> Result<(), String> {
        let notes = take(&mut self.landing.notes);
        self.landing.lost.clear();
        match notes.is_empty() {
            true => Ok(()),
            false => Err(notes.join("; ")),
        }
    }

    /// The last line's result when its file changes did not land, once.
    pub(crate) fn take_not_landed_line(&mut self) -> Option<String> {
        self.landing.line.take()
    }
}

/// A "Done: a; b. undo" line less the changes in `lost`; `None` when none
/// is left.
pub(crate) fn without_lost(line: &str, lost: &[String]) -> Option<String> {
    let Some((lead, said)) = line.split_once(": ") else {
        return Some(String::from(line));
    };
    let (said, tail) = match said.strip_suffix(". undo") {
        Some(said) => (said, ". undo"),
        None => (said, ""),
    };
    let mut left: Vec<&str> = said.split("; ").collect();
    for what in lost {
        if let Some(at) = left.iter().position(|w| w == what) {
            left.remove(at);
        }
    }
    match left.is_empty() {
        true => None,
        false => Some(format!("{lead}: {}{tail}", left.join("; "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn a_done_line_loses_only_the_changes_that_did_not_land() {
        let lost = vec![String::from("file a.txt written")];
        assert_eq!(
            without_lost(
                "Done: policy net on (was off); file a.txt written. undo",
                &lost
            ),
            Some(String::from("Done: policy net on (was off). undo"))
        );
        assert_eq!(without_lost("Done: file a.txt written. undo", &lost), None);
        assert_eq!(
            without_lost("Taken as: file b.txt written", &lost).as_deref(),
            Some("Taken as: file b.txt written")
        );
    }
}
