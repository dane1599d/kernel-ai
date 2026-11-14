//! A line typed in the desktop's AI panel (P3 task 7). It runs as a terminal
//! line does, with these differences, because the panel's bar shows only a
//! one-line state and is drawn by `desktop` as the system's own:
//!
//! - the terminal's open entry, clarifying question and proposal are its own: a
//!   panel line neither answers nor continues them, and what a panel line
//!   leaves open (a question, an open bracket) is dropped, since the panel
//!   cannot show it yet;
//! - what the bar shows is code's: the result line of what ran, or a fixed
//!   reason. The model's words, a model-written script's prints and its
//!   summary are never shown there. They go to the desk's history above the
//!   bar instead ([`Shell::take_panel_said`]), which `desktop` draws as the
//!   AI's, apart from the system's look (the founder, 2026-11-14);
//! - C is the terminal's (P4 task 12): a panel line runs at `kai>` even when
//!   the terminal is at `c>`, and `:c` and `:rhai` are refused. While a C
//!   entry waits on its question, a panel line is refused: the entry's log
//!   entry takes in what is logged after it began.

use crate::{Reply, Shell};
use alloc::string::String;

/// What the panel shows for a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelResult {
    Done(String),
    Refused(String),
}

const ASKS: &str = "it asks first: ask in the terminal, which shows the script";
const WORDS: &str = "it asked a question: answer it in the terminal";
/// The bar's line when the model answered in words: they are in the history.
const ANSWERED: &str = "Kai answered above.";
/// The most of what the AI said that the history is given.
pub const MAX_SAID: usize = 4000;
const OPEN: &str = "a bracket is still open: type the whole line in the terminal";
const FAILED: &str = "it did not run: `show` in the terminal says why";
const C_WAITS: &str = "a C entry waits for an answer: answer it in the terminal first";
const C_LINE: &str = "C goes in the terminal";

/// What a panel line may change of the terminal's own state, set aside. An
/// app's terminal (`appline`) keeps one of its own.
#[derive(Default)]
pub(crate) struct Terminal {
    pub(crate) partial: Option<String>,
    pub(crate) clarifying: Option<String>,
    pub(crate) pending: Option<crate::actions::Pending>,
    pub(crate) undo_asking: Option<u64>,
    pub(crate) c: crate::cmode::CMode,
}

impl Shell {
    /// Runs one panel line: what the panel shows.
    pub fn eval_panel(&mut self, line: &str) -> PanelResult {
        let line = line.trim();
        if line.starts_with("answer ") {
            return PanelResult::Refused(String::from("answers go in the terminal"));
        }
        if self.c_waiting() {
            return PanelResult::Refused(String::from(C_WAITS));
        }
        if line == ":c" || line.starts_with(":c ") || line == ":rhai" {
            return PanelResult::Refused(String::from(C_LINE));
        }
        let terminal = Terminal {
            partial: self.partial.take(),
            clarifying: self.clarifying.take(),
            pending: self.pending.take(),
            undo_asking: self.undo_asking.take(),
            c: core::mem::take(&mut self.c),
        };
        self.asked_model = false;
        self.worded = false;
        self.done_line = None;
        self.panel_said = None;
        let reply = self.eval(line);
        // Before the result takes the done line, which `said` leaves out.
        self.panel_said = self.said(&reply);
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
        self.c = terminal.c;
        result
    }

    /// What the AI said to the last panel line, once: the model's words, or
    /// the prints of the script it wrote, for the desk's history. `None` for
    /// the person's own lines and for what said nothing.
    pub fn take_panel_said(&mut self) -> Option<String> {
        self.panel_said.take()
    }

    /// The reply's text when the model made it: its words (an answer, or a
    /// question left for the terminal), or what its script printed, without
    /// the result line the bar shows. Cut to [`MAX_SAID`] bytes.
    fn said(&self, reply: &Reply) -> Option<String> {
        // A line that asks first shows its script in the terminal, not here.
        if !self.asked_model || reply.confirm.is_some() || self.pending.is_some() {
            return None;
        }
        if !self.worded && self.clarifying.is_none() && !reply.ok {
            return None;
        }
        let done = self.done_line.as_deref().map(without_undo);
        let kept: alloc::vec::Vec<&str> = reply
            .text
            .lines()
            .filter(|line| done.as_deref() != Some(without_undo(line).as_str()))
            .collect();
        let text = kept.join("\n");
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let mut end = text.len().min(MAX_SAID);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Some(String::from(&text[..end]))
    }

    fn panel_result(&mut self, reply: &Reply) -> PanelResult {
        let refused = |why: &str| PanelResult::Refused(String::from(why));
        if reply.confirm.is_some() || self.pending.is_some() || self.undo_asking.is_some() {
            return refused(ASKS);
        }
        if reply.more {
            return refused(OPEN);
        }
        if self.clarifying.is_some() {
            return refused(WORDS);
        }
        if self.worded {
            return PanelResult::Done(String::from(ANSWERED));
        }
        if self.asked_model {
            return match reply.ok {
                true => PanelResult::Done(without_undo(
                    &self
                        .done_line
                        .take()
                        .unwrap_or_else(|| String::from("Done.")),
                )),
                false => refused(FAILED),
            };
        }
        // The user's own line: its result is what they asked to see.
        let first = reply.text.lines().find(|line| !line.trim().is_empty());
        match (reply.ok, first) {
            (true, first) => PanelResult::Done(without_undo(first.unwrap_or("Done."))),
            (false, Some(why)) => PanelResult::Refused(String::from(why)),
            (false, None) => refused(FAILED),
        }
    }
}

/// A done line as the panel shows it: desktop draws its own undo control
/// beside it, so the console's trailing `. undo` goes.
fn without_undo(line: &str) -> String {
    match line.strip_suffix(". undo") {
        Some(said) => alloc::format!("{said}."),
        None => String::from(line),
    }
}
