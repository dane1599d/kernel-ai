//! A line typed in the desktop's AI panel (P3 task 7). It runs as a terminal
//! line does, with two differences, because the panel shows only a one-line
//! state and is drawn by `desktop` as the system's own:
//!
//! - the terminal's open entry, clarifying question and proposal are its own: a
//!   panel line neither answers nor continues them, and what a panel line
//!   leaves open (a question, an open bracket) is dropped, since the panel
//!   cannot show it yet;
//! - what the panel shows is code's: the result line of what ran, or a fixed
//!   reason. The model's words, a model-written script's prints and its
//!   summary are never shown there.

use crate::{Reply, Shell};
use alloc::string::String;

/// What the panel shows for a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelResult {
    Done(String),
    Refused(String),
}

const ASKS: &str = "it asks first: ask in the terminal, which shows the script";
const WORDS: &str = "it answered in words: ask in the terminal to read them";
const OPEN: &str = "a bracket is still open: type the whole line in the terminal";
const FAILED: &str = "it did not run: `show` in the terminal says why";

/// What a panel line may change of the terminal's own state, set aside.
struct Terminal {
    partial: Option<String>,
    clarifying: Option<String>,
    pending: Option<crate::actions::Pending>,
    undo_asking: Option<u64>,
}

impl Shell {
    /// Runs one panel line: what the panel shows.
    pub fn eval_panel(&mut self, line: &str) -> PanelResult {
        let line = line.trim();
        if line.starts_with("answer ") {
            return PanelResult::Refused(String::from("answers go in the terminal"));
        }
        let terminal = Terminal {
            partial: self.partial.take(),
            clarifying: self.clarifying.take(),
            pending: self.pending.take(),
            undo_asking: self.undo_asking.take(),
        };
        self.asked_model = false;
        self.worded = false;
        self.done_line = None;
        let reply = self.eval(line);
        let result = self.panel_result(&reply);
        if let Some(left) = self.pending.take() {
            let outcome = "discarded: the panel cannot ask yet";
            self.record(left.id, &left.request, &left.checked.script, outcome);
            self.save_log();
        }
        self.partial = terminal.partial;
        self.clarifying = terminal.clarifying;
        self.pending = terminal.pending;
        self.undo_asking = terminal.undo_asking;
        result
    }

    fn panel_result(&mut self, reply: &Reply) -> PanelResult {
        let refused = |why: &str| PanelResult::Refused(String::from(why));
        if reply.confirm.is_some() || self.pending.is_some() || self.undo_asking.is_some() {
            return refused(ASKS);
        }
        if reply.more {
            return refused(OPEN);
        }
        if self.worded || self.clarifying.is_some() {
            return refused(WORDS);
        }
        if self.asked_model {
            return match reply.ok {
                true => PanelResult::Done(
                    self.done_line
                        .take()
                        .unwrap_or_else(|| String::from("Done.")),
                ),
                false => refused(FAILED),
            };
        }
        // The user's own line: its result is what they asked to see.
        let first = reply.text.lines().find(|line| !line.trim().is_empty());
        match (reply.ok, first) {
            (true, first) => PanelResult::Done(String::from(first.unwrap_or("Done."))),
            (false, Some(why)) => PanelResult::Refused(String::from(why)),
            (false, None) => refused(FAILED),
        }
    }
}
