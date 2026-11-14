//! A line typed in the desktop's AI panel (P3 task 7). It runs as a terminal
//! line does, with these differences, because the panel's bar shows only a
//! one-line state and is drawn by `desktop` as the system's own:
//!
//! - the terminal's open entry, clarifying question and proposal are its own: a
//!   panel line neither answers nor continues them. A question a panel line
//!   leaves is the panel's own, kept for its next line, which answers it, as
//!   an app's terminal keeps its own; an open bracket is dropped. A proposal
//!   the trusted card can show waits for it instead (`deskask`), and the
//!   person answers there;
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
/// The bar's line when the model asked a question: it is in the history.
const ASKED: &str = "Kai asked a question above: answer it here";
/// The bar's line when code asked which one: the question is in the history.
const WHICH: &str = "A question above: answer it here";
/// The history's note when code asked which one. Never the question itself:
/// it echoes a name the model chose, unchecked, and the note is drawn in the
/// system's look.
const WHICH_NOTE: &str = "Which one do you mean? Name it in your answer.";
/// The bar's line when the model answered in words: they are in the history.
const ANSWERED: &str = "Kai answered above.";
/// The most of what the AI said that the history is given.
pub const MAX_SAID: usize = 4000;
const OPEN: &str = "a bracket is still open: type the whole line in the terminal";
pub(crate) const FAILED: &str = "it did not run: `show` in the terminal says why";
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
    /// The log entry the last panel line made, once: the entry its undo link
    /// undoes (`undo <id>`), never the newest of another line's. `None` when
    /// it changed nothing that `undo` takes back.
    pub fn take_panel_entry(&mut self) -> Option<u64> {
        self.panel_entry.take()
    }

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
        let mut terminal = Terminal {
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
        self.panel_note = None;
        self.clarified_by_model = false;
        // The panel's own question, if its last line left one: this line
        // answers it.
        self.clarifying = self.panel_clarifying.take();
        self.forgot = false;
        let before = self.log.newest_live();
        let reply = self.eval(line);
        let after = self.log.newest_live();
        self.panel_entry = (after != before).then_some(after).flatten();
        // `new` typed here forgets the terminal's question too: one
        // conversation, wherever it was typed.
        if self.forgot {
            terminal.clarifying = None;
        }
        // Before the result takes the done line, which `said` leaves out.
        self.panel_said = self.said(&reply);
        // A question the card can show waits for it (`deskask`).
        let result = match reply.confirm.is_some() && self.pending.is_some() {
            true => match self.wait_for_card(&reply) {
                Ok(waits) => PanelResult::Done(String::from(waits)),
                Err(why) => PanelResult::Refused(String::from(why)),
            },
            false => self.panel_result(&reply),
        };
        if let Some(left) = self.pending.take() {
            let outcome = "discarded: the panel cannot ask yet";
            self.record(left.id, &left.request, &left.checked.script, outcome);
            self.save_log();
        }
        self.partial = terminal.partial;
        self.panel_clarifying = self.clarifying.take();
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

    /// A question code asked the last panel line, once: which target it
    /// means. It goes to the desk's history as code's note, never to the bar,
    /// since it can carry a name the model chose.
    pub fn take_panel_note(&mut self) -> Option<String> {
        self.panel_note.take()
    }

    /// The reply's text when the model wrote it: its words (an answer, or its
    /// own question), or what its script printed. Code's lines stay out: the
    /// run's result line, "nothing was changed", the "Done: summary" written
    /// when a run printed nothing, a question code asks (which target). Cut to
    /// [`MAX_SAID`] bytes.
    fn said(&self, reply: &Reply) -> Option<String> {
        // A line that asks first shows its script on the card, not here.
        if !self.asked_model || reply.confirm.is_some() || self.pending.is_some() {
            return None;
        }
        if self.clarifying.is_some() && !self.clarified_by_model {
            return None;
        }
        if !self.worded && self.clarifying.is_none() && !reply.ok {
            return None;
        }
        let done = self.done_line.as_deref().map(without_undo);
        let ours = |line: &str| {
            line == crate::guard::NOTHING_CHANGED
                || done.as_deref() == Some(without_undo(line).as_str())
        };
        let mut kept: alloc::vec::Vec<&str> = match self.worded || self.clarified_by_model {
            true => reply.text.lines().collect(),
            false => reply.text.lines().filter(|line| !ours(line)).collect(),
        };
        // What the shell writes for a run that printed nothing ("Done.", or
        // "Done: " and the model's summary) is code's framing, not Kai's words.
        if !self.worded
            && !self.clarified_by_model
            && let [only] = kept.as_slice()
            && (*only == "Done." || only.starts_with("Done: "))
        {
            kept.clear();
        }
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
            // The question is in the history: the model's as Kai's words,
            // code's as a note. The bar's line is fixed, and the next panel
            // line answers it.
            if self.clarified_by_model {
                return PanelResult::Done(String::from(ASKED));
            }
            self.panel_note = Some(String::from(WHICH_NOTE));
            return PanelResult::Done(String::from(WHICH));
        }
        if self.worded {
            return PanelResult::Done(String::from(ANSWERED));
        }
        if self.asked_model {
            // A run that changed nothing says so here, never "Done.".
            let nothing = reply.text.lines().last() == Some(crate::guard::NOTHING_CHANGED);
            return match (reply.ok, nothing) {
                (true, true) => PanelResult::Done(String::from(crate::guard::NOTHING_CHANGED)),
                (true, false) => PanelResult::Done(without_undo(
                    &self
                        .done_line
                        .take()
                        .unwrap_or_else(|| String::from("Done.")),
                )),
                (false, _) => refused(FAILED),
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
pub(crate) fn without_undo(line: &str) -> String {
    match line.strip_suffix(". undo") {
        Some(said) => alloc::format!("{said}."),
        None => String::from(line),
    }
}
