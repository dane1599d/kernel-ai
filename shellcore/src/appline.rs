//! A line typed in an app's window: the Terminal's `shell_line` effect (P4 task
//! 4). It runs as a terminal line does, with differences that follow from who
//! asks:
//!
//! - **It has a terminal of its own.** The open entry (`..> `), the clarifying
//!   question and the proposal are the app's, kept between lines, and the
//!   console's are set aside while it runs, as the AI panel's are. A line typed
//!   here neither answers nor continues the console's.
//! - **It cannot ask.** What would wait for a yes (a script the model proposed,
//!   an `undo` that needs confirming) is refused with a reason, and the proposal
//!   is dropped, as the panel's is: an app cannot ask on its own behalf, and
//!   only the console and the desktop's card answer (ADR 0005 section 7). What
//!   runs at once, local and undoable, is logged and undone like any line.
//! - **Only the Terminal may send one**, and only for a person's key or click.
//!   `shell_line` is not a registry row, so no tier decides it, and no package
//!   may declare it: the grant is the process the image's plan line names
//!   [`TERMINAL_APP`], a name no package can take (a package name is at most 12
//!   bytes). A line queued by an action call, a reply or `init` is refused
//!   ([`Shell::run_effects_from`]), so the AI cannot type for the founder.
//!
//! The answer is the shell's wire reply (`ok ...`, `err ...` or `more`) as one
//! line, with `\` and newlines escaped (`\\`, `\n`), because an effect's result
//! is one line of at most 200 bytes (`deskcore::effects::MAX_TEXT`); a longer answer is cut
//! at a character with a `…`.

use crate::{Reply, Shell};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use appabi::{Ty, Val, wave};

/// The effect's name.
pub(crate) const SHELL_LINE: &str = "shell_line";
/// The app that may run shell lines: the Terminal's component.
pub const TERMINAL_APP: &str = "terminal-window";
/// Why a `shell_line` that did not come from a person's input is refused.
pub(crate) const NOT_BY_A_PERSON: &str = "shell_line runs only for a key or click in the window";
/// Why a line is refused while the console's C entry waits on its question.
const C_WAITS: &str = "a C entry waits for an answer: answer it in the console first";
/// Why `:c` and `:rhai` are refused: C goes in the console.
const C_LINE: &str = "C goes in the console";
/// What an app that asked gets back, in place of the question.
const ASKS: &str = "it asks first: ask in the console, which shows the script";
/// Most bytes of an answer sent back, so that with its `…` it fits the most
/// the transport carries of a person's result (`deskcore::effects::MAX_LINE_TEXT`,
/// which `user`'s `shell` checks at compile time).
pub const MAX_ANSWER: usize = 2038;

/// The line of `shell_line({line: "..."})`.
fn line_of(args: &str) -> Result<String, String> {
    let ty = Ty::Record(vec![(String::from("line"), Ty::String)]);
    match wave::parse(args, &ty) {
        Ok(Val::Record(fields)) => match fields.as_slice() {
            [Val::String(line)] => Ok(line.clone()),
            _ => Err(String::from("shell_line takes {line}")),
        },
        Ok(_) => Err(String::from("shell_line takes {line}")),
        Err(why) => Err(format!("shell_line's arguments: {why}")),
    }
}

/// `wire` as one line: `\` and newlines escaped, cut to [`MAX_ANSWER`] bytes at
/// a character (and never inside an escape) with a `…`.
fn one_line(wire: &str) -> String {
    let mut out = String::new();
    let mut cut = false;
    for ch in wire.chars() {
        let piece: &str = match ch {
            '\\' => "\\\\",
            '\n' => "\\n",
            '\t' => "\\t",
            _ => "",
        };
        let width = if piece.is_empty() { ch.len_utf8() } else { 2 };
        if out.len() + width > MAX_ANSWER {
            cut = true;
            break;
        }
        match piece.is_empty() {
            true => out.push(ch),
            false => out.push_str(piece),
        }
    }
    if cut {
        out.push('…');
    }
    out
}

impl Shell {
    /// `shell_line` for `app`: its line, run in its terminal. The answer, or why
    /// the line was not taken.
    pub(crate) fn run_shell_line(&mut self, app: &str, args: &str) -> Result<String, String> {
        if app != TERMINAL_APP {
            return Err(format!("{app} may not run shell lines"));
        }
        let line = line_of(args)?;
        self.host.audit(&format!("{app} ran the line {line:?}"));
        Ok(one_line(&self.eval_app(app, &line).to_wire()))
    }

    /// Runs `line` in `app`'s terminal state, the console's set aside.
    fn eval_app(&mut self, app: &str, line: &str) -> Reply {
        let line = line.trim();
        if line.starts_with("answer ") {
            return Reply::err("answers go on the question's own prompt");
        }
        // While the console's C entry waits on its question, its log entry
        // takes in whatever is logged after it began: no line runs between.
        if self.c_waiting() {
            return Reply::err(C_WAITS);
        }
        // C is the console's: an app line is always Rhai, and does not switch.
        if line == ":c" || line.starts_with(":c ") || line == ":rhai" {
            return Reply::err(C_LINE);
        }
        let console = crate::panel::Terminal {
            partial: self.partial.take(),
            clarifying: self.clarifying.take(),
            pending: self.pending.take(),
            undo_asking: self.undo_asking.take(),
            c: core::mem::take(&mut self.c),
        };
        // The console's tune proposal is its own: a line here neither answers
        // it nor lapses it, and one it leaves is dropped below.
        let console_tune = self.tuner.set_aside();
        let mine = self.fronts.remove(app).unwrap_or_default();
        self.partial = mine.partial;
        self.clarifying = mine.clarifying;
        self.pending = mine.pending;
        self.undo_asking = mine.undo_asking;
        self.c = mine.c;
        self.asked_model = false;
        self.worded = false;
        self.done_line = None;
        let mut reply = self.eval(line);
        if reply.confirm.is_some() || self.pending.is_some() || self.undo_asking.is_some() {
            if let Some(left) = self.pending.take() {
                let outcome = "discarded: the terminal app cannot ask yet";
                self.record(left.id, &left.request, &left.checked.script, outcome);
                self.save_log();
            }
            self.undo_asking = None;
            reply = Reply::err(ASKS);
        }
        self.tuner.discard();
        self.tuner.restore(console_tune);
        let kept = crate::panel::Terminal {
            partial: self.partial.take(),
            clarifying: self.clarifying.take(),
            pending: None,
            undo_asking: None,
            c: core::mem::take(&mut self.c),
        };
        self.fronts.insert(String::from(app), kept);
        self.partial = console.partial;
        self.clarifying = console.clarifying;
        self.pending = console.pending;
        self.undo_asking = console.undo_asking;
        self.c = console.c;
        reply
    }
}
