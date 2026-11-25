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
    /// The note's own lines in the last reply, which the panel's history
    /// leaves out of what the line said.
    said_note: Vec<String>,
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
        self.landing.said_note.clear();
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
        // The done line may hold a line break (a file name's), so it is
        // swapped in the whole text, not line by line.
        let mut text = reply.text.clone();
        if let Some(done) = done.as_deref()
            && let Some(at) = text.find(done)
        {
            let mut end = at + done.len();
            if rest.is_none() && text[end..].starts_with('\n') {
                end += 1;
            }
            text.replace_range(at..end, rest.as_deref().unwrap_or(""));
        }
        let mut lines: Vec<&str> = text.lines().collect();
        // A question still waiting stays last.
        match reply.confirm {
            Some(_) => lines.insert(0, &note),
            None => lines.push(&note),
        }
        // The row reads "Done: … (was x); #1: …": no full stop before the ";".
        self.landing.line = Some(match &rest {
            Some(rest) => format!(
                "{}; {note}",
                crate::panel::without_undo(rest).trim_end_matches('.')
            ),
            None => note.clone(),
        });
        self.landing.said_note = note.lines().map(String::from).collect();
        self.done_line = rest;
        Reply {
            ok: false,
            text: lines.join("\n"),
            ..reply
        }
    }

    /// Whether `line` is one of the note's own: the panel's history leaves it
    /// out of what the line said, but keeps the line's output.
    pub(crate) fn is_not_landed_note(&self, line: &str) -> bool {
        self.landing.said_note.iter().any(|n| n == line)
    }

    /// Whether the last line's file changes did not land.
    pub(crate) fn line_did_not_land(&self) -> bool {
        self.landing.line.is_some()
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
/// is left. Each change goes as a whole item, so a name holding "; " goes
/// with it.
pub(crate) fn without_lost(line: &str, lost: &[String]) -> Option<String> {
    let Some((lead, said)) = line.split_once(": ") else {
        return Some(String::from(line));
    };
    let (said, tail) = match said.strip_suffix(". undo") {
        Some(said) => (said, ". undo"),
        None => (said, ""),
    };
    let mut left = String::from(said);
    for what in lost {
        left = without_item(&left, what);
    }
    match left.is_empty() {
        true => None,
        false => Some(format!("{lead}: {left}{tail}")),
    }
}

/// `list` ("a; b; c") less the item `what`, where it stands whole between
/// the start, "; " and the end.
fn without_item(list: &str, what: &str) -> String {
    let mut from = 0;
    while let Some(found) = list[from..].find(what) {
        let (start, end) = (from + found, from + found + what.len());
        let before = start == 0 || list[..start].ends_with("; ");
        let after = end == list.len() || list[end..].starts_with("; ");
        if before && after {
            let (cut_from, cut_to) = match (start == 0, end == list.len()) {
                (_, true) if start == 0 => (0, end),
                (_, true) => (start - 2, end),
                _ => (start, end + 2),
            };
            return format!("{}{}", &list[..cut_from], &list[cut_to..]);
        }
        from = start + 1;
        while !list.is_char_boundary(from) {
            from += 1;
        }
    }
    String::from(list)
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
        // A name with "; " in it goes whole.
        let odd = vec![String::from("file x; y.txt written")];
        assert_eq!(
            without_lost("Done: policy net on; file x; y.txt written. undo", &odd),
            Some(String::from("Done: policy net on. undo"))
        );
        assert_eq!(
            without_lost("Done: file x; y.txt written; policy net on. undo", &odd),
            Some(String::from("Done: policy net on. undo"))
        );
        assert_eq!(
            without_lost("Taken as: file b.txt written", &lost).as_deref(),
            Some("Taken as: file b.txt written")
        );
    }
}
