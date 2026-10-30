//! The Terminal app's model (P4 task 4, ADR 0005 section 4): today's shell in an
//! app window. The shell is the same ring-3 service the kernel terminal talks
//! to; this model keeps what the window shows and the rules for the input line,
//! and the window (`comp-terminal-ui`) draws it.
//!
//! - **Typed spans.** Every row of the scrollback is typed by its origin
//!   ([`span::Kind`]), never by its text, so no output can pass for an answer, an
//!   error or a question.
//! - **Input.** A line is sent to the shell, which replies `more` while a bracket
//!   is open; the prompt then reads `..> ` until the entry is complete. The
//!   shell may also report its own prompt with a reply (`kai> `, or `c> ` in the
//!   compiler mode, ADR 0012): [`Terminal::reply`] takes it as a hook.
//! - **Questions are not answered here.** A `confirm` or `menu` is shown as a
//!   question, and the terminal takes no line until the trusted path has
//!   answered it (ADR 0005 section 7). The one thing a window may do is
//!   decline: [`Terminal::cancel`].
//! - **The AI's hands.** `terminal_read` is [`Terminal::read`]. `terminal_run`
//!   only types: [`Terminal::stage`] puts a line in the input for a person to
//!   run with Enter, because the shell runs a line only for a person's key or
//!   click. The row then says the AI typed it. This is the founder's terminal.

#![no_std]

extern crate alloc;

pub mod os;
pub mod span;
pub mod state;
pub mod wire;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use os::{Os, SHELL_LINE, shell_line_args, unescape};
use span::{Kind, MAX_SPANS, Span, clean, fit};
use termcore::menu::{Answer, answer_line};
use wire::Wire;

/// Longest line the prompt takes, in characters (the kernel terminal's).
pub const MAX_LINE: usize = 200;
/// Longest line in bytes, so that its effect (`os::shell_line_args`, where a
/// quote or a backslash takes two bytes) fits the 512 bytes an effect may carry.
pub const MAX_SENT_BYTES: usize = 250;
/// The prompt before the shell reports another.
pub const DEFAULT_PROMPT: &str = "kai> ";
/// The prompt for the rest of an open entry.
pub const CONTINUE_PROMPT: &str = "..> ";
/// Longest prompt a shell may report, with its `> `.
pub const MAX_PROMPT: usize = 8;
/// What the shell takes as "drop the open entry" (`shellcore::CANCEL_ENTRY`).
pub const CANCEL_ENTRY: &str = "\u{3}";
/// What the terminal says when it starts.
pub const GREETING: &str = "Kernel AI terminal. Talk to the AI or type Rhai (try help). \
                            Kernel commands that start with ':' run on the console.";
/// Said under a question.
pub const ANSWER_ELSEWHERE: &str = "Answer on the AI panel's card.";
/// Most rows `terminal_read` returns, newest last.
pub const READ_MAX_SPANS: usize = 60;
/// Most bytes of text `terminal_read` returns.
pub const READ_MAX_BYTES: usize = 6 * 1024;

/// Where the next line goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// A new line may be typed.
    Ready,
    /// The shell holds an open entry: the next line continues it.
    Continuing,
    /// A line is with the shell, and its answer has not come.
    Waiting,
    /// The shell asked, and only the trusted path answers.
    Asking(Ask),
}

impl Phase {
    /// The word `terminal_read` says.
    pub const fn word(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Continuing => "continuing",
            Self::Waiting => "waiting",
            Self::Asking(_) => "asking",
        }
    }
}

/// A question the shell waits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ask {
    pub id: u64,
    /// Whether it is an Allow / Deny menu rather than a script's yes.
    pub menu: bool,
}

impl Ask {
    /// The line that declines it, the one answer a window may give.
    fn decline(self) -> String {
        match self.menu {
            true => answer_line(self.id, Answer::Deny),
            false => format!("answer {} no", self.id),
        }
    }
}

/// Who typed a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Typed,
    /// A line run for the AI by a host that lets it. The component does not: its
    /// lines are a person's, marked "by the AI" only when the AI typed them
    /// ([`Terminal::stage`]).
    Ai,
}

/// Why a line was not taken. Each also leaves a notice in the scrollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The last line is still with the shell.
    Busy,
    /// A question waits for the trusted path.
    Asking,
    TooLong,
    /// A control character, a newline included: one line at a time.
    Control,
    /// It reads like `answer <id> ...`, which only the trusted path sends.
    Answer,
    /// Nothing to type.
    Empty,
    /// A line the AI types is plain, so a person sees all of what Enter runs.
    Hidden,
}

impl Refusal {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Busy => "wait: the last line is still running",
            Self::Asking => "a question is waiting; answer it on the AI panel's card",
            Self::TooLong => "that line is longer than 200 characters or 250 bytes",
            Self::Control => "a line holds no control characters or newlines",
            Self::Answer => "answers are given only on a question's own card",
            Self::Empty => "there is nothing to type",
            Self::Hidden => {
                "a line the AI types is printable ASCII with single spaces, at most 160 characters"
            }
        }
    }
}

/// What [`Terminal::submit`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Submitted {
    /// An empty line at a fresh prompt.
    Nothing,
    /// The terminal handled it itself (`:clear`).
    Local,
    /// Send this line to the shell; the reply comes to [`Terminal::reply`].
    Send(String),
    Refused(Refusal),
}

pub struct Terminal {
    spans: Vec<Span>,
    next_seq: u32,
    base: String,
    phase: Phase,
    /// The ticket of the effect the shell has not answered. It is not saved:
    /// an effect does not outlive the app that queued it.
    in_flight: Option<u32>,
    /// The line the AI typed into the input, until a person runs or changes it.
    /// Not saved.
    staged: Option<String>,
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

impl Terminal {
    /// A terminal that has just said hello.
    pub fn new() -> Self {
        let mut t = Self {
            spans: Vec::new(),
            next_seq: 1,
            base: String::from(DEFAULT_PROMPT),
            phase: Phase::Ready,
            in_flight: None,
            staged: None,
        };
        t.push(Kind::Notice, "", GREETING, false);
        t
    }

    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The sequence number the next row gets.
    pub fn next_seq(&self) -> u32 {
        self.next_seq
    }

    /// The prompt in front of the next line.
    pub fn prompt(&self) -> &str {
        match self.phase {
            Phase::Continuing => CONTINUE_PROMPT,
            _ => &self.base,
        }
    }

    /// Whether a line may be typed now.
    pub fn takes_input(&self) -> bool {
        matches!(self.phase, Phase::Ready | Phase::Continuing)
    }

    fn push(&mut self, kind: Kind, prompt: &str, text: &str, by_ai: bool) {
        let span = Span {
            seq: self.next_seq,
            kind,
            prompt: String::from(prompt),
            text: String::from(text),
            by_ai,
        };
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        self.spans.push(span);
        if self.spans.len() > MAX_SPANS {
            self.spans.remove(0);
        }
    }

    /// Says why a line was not taken, in the scrollback.
    fn note(&mut self, why: Refusal) -> Refusal {
        self.push(Kind::Notice, "", why.message(), false);
        why
    }

    fn refuse(&mut self, why: Refusal) -> Submitted {
        Submitted::Refused(self.note(why))
    }

    /// `terminal_run`: the AI types `line` into the input, for a person to run
    /// with Enter. Nothing is sent to the shell, and a person may change or drop
    /// the line first. The line as it will show in the input.
    pub fn stage(&mut self, line: &str) -> Result<String, Refusal> {
        match self.phase {
            Phase::Waiting => return Err(self.note(Refusal::Busy)),
            Phase::Asking(_) => return Err(self.note(Refusal::Asking)),
            Phase::Ready | Phase::Continuing => {}
        }
        let line = line.trim();
        if line.is_empty() {
            return Err(self.note(Refusal::Empty));
        }
        if let Some(why) = vet(line) {
            return Err(self.note(why));
        }
        if !plain(line) {
            return Err(self.note(Refusal::Hidden));
        }
        self.staged = Some(String::from(line));
        // The whole line, in the notice: a person who presses Enter has seen
        // what runs, wherever the input field cuts it.
        self.push(Kind::Notice, "", &format!("{STAGED}: {line}"), false);
        Ok(String::from(line))
    }

    /// A line typed at the prompt and run with Enter. It is echoed, and unless
    /// the terminal handles it ([`Submitted::Local`]) the caller sends it to the
    /// shell.
    pub fn submit(&mut self, line: &str, source: Source) -> Submitted {
        match self.phase {
            Phase::Waiting => return self.refuse(Refusal::Busy),
            Phase::Asking(_) => return self.refuse(Refusal::Asking),
            Phase::Ready | Phase::Continuing => {}
        }
        let line = line.trim();
        let continuing = self.phase == Phase::Continuing;
        if line.is_empty() && !continuing {
            return Submitted::Nothing;
        }
        if let Some(why) = vet(line) {
            return self.refuse(why);
        }
        if !continuing && line == ":clear" {
            self.clear();
            return Submitted::Local;
        }
        let prompt = String::from(self.prompt());
        let shown = fit(&clean(line).lines.join(" "));
        // A line the AI typed and a person ran as it was is the AI's.
        let typed_by_ai = self.staged.take().is_some_and(|staged| staged == line);
        self.push(
            Kind::Input,
            &prompt,
            &shown,
            source == Source::Ai || typed_by_ai,
        );
        self.phase = Phase::Waiting;
        Submitted::Send(String::from(line))
    }

    /// The shell's reply to the line in flight (or, for a question, the answer's
    /// outcome). `prompt` is the shell's own prompt if it reports one, which a
    /// compiler mode uses (`c> `); it is taken only if it is short, plain ASCII
    /// and ends in `> `. Returns a line to send back at once, only the decline
    /// of a menu that could not be shown.
    pub fn reply(&mut self, wire: &str, prompt: Option<&str>) -> Option<String> {
        if let Some(prompt) = prompt.filter(|p| valid_prompt(p)) {
            self.base = String::from(prompt);
        }
        match wire::parse(wire) {
            Wire::More => self.phase = Phase::Continuing,
            Wire::Ok(text) => self.said(Kind::Reply, text),
            Wire::Err(text) => self.said(Kind::Error, text),
            Wire::Confirm { id, text } => {
                self.said(Kind::Question, text);
                self.asked(Ask { id, menu: false });
            }
            Wire::Menu(menu) => {
                self.said(Kind::Question, menu.question());
                self.asked(Ask {
                    id: menu.id,
                    menu: true,
                });
            }
            Wire::BadMenu(id) => {
                self.said(
                    Kind::Error,
                    "the shell sent a question that cannot be shown",
                );
                self.phase = Phase::Ready;
                if let Some(id) = id {
                    self.phase = Phase::Waiting;
                    return Some(answer_line(id, Answer::Deny));
                }
            }
            Wire::Malformed => self.said(Kind::Error, "the shell sent a malformed reply"),
        }
        None
    }

    /// Shows `text` as rows of `kind`, and returns to the prompt.
    fn said(&mut self, kind: Kind, text: &str) {
        let cleaned = clean(text);
        for line in &cleaned.lines {
            self.push(kind, "", line, false);
        }
        if cleaned.cut {
            self.push(Kind::Notice, "", "(the rest was left out)", false);
        }
        self.phase = Phase::Ready;
    }

    fn asked(&mut self, ask: Ask) {
        self.push(Kind::Notice, "", ANSWER_ELSEWHERE, false);
        self.phase = Phase::Asking(ask);
    }

    /// Ctrl-C or the Cancel button. At `..> ` it drops the open entry; at a
    /// question it declines it. The line to send to the shell, if any.
    pub fn cancel(&mut self) -> Option<String> {
        let line = match self.phase {
            Phase::Continuing => String::from(CANCEL_ENTRY),
            Phase::Asking(ask) => ask.decline(),
            Phase::Ready | Phase::Waiting => return None,
        };
        self.push(Kind::Notice, "", "^C", false);
        self.phase = Phase::Waiting;
        Some(line)
    }

    /// Drops the scrollback. Numbers keep counting, so a reader that asks for
    /// what follows its last row gets what is new.
    pub fn clear(&mut self) {
        self.spans.clear();
    }

    /// Queues `line` for the shell and remembers its ticket.
    fn queue(&mut self, os: &mut dyn Os, line: &str) {
        self.in_flight = Some(os.request(SHELL_LINE, shell_line_args(line)));
    }

    /// [`Terminal::submit`], then the effect that sends the line to the shell.
    /// A person's Enter and the AI's `terminal_run` both come here.
    pub fn run(&mut self, os: &mut dyn Os, line: &str, source: Source) -> Submitted {
        let submitted = self.submit(line, source);
        if let Submitted::Send(line) = &submitted {
            self.queue(os, line);
        }
        submitted
    }

    /// [`Terminal::cancel`], then the effect that sends its line; whether there
    /// was one.
    pub fn stop(&mut self, os: &mut dyn Os) -> bool {
        match self.cancel() {
            Some(line) => {
                self.queue(os, &line);
                true
            }
            None => false,
        }
    }

    /// The result of the effect with `ticket`: the shell's wire reply, or why the
    /// OS did not run the line. Whether it was ours and the window changed.
    pub fn done(&mut self, os: &mut dyn Os, ticket: u32, outcome: Result<String, String>) -> bool {
        if self.in_flight != Some(ticket) {
            return false;
        }
        self.in_flight = None;
        let back = match outcome {
            Ok(wire) => self.reply(&unescape(&wire), None),
            Err(why) => {
                self.said(Kind::Error, &format!("the OS did not run it: {why}"));
                None
            }
        };
        if let Some(line) = back {
            self.queue(os, &line);
        }
        true
    }

    /// What `terminal_read` returns: `next <seq> <phase>` (the number to ask
    /// from next time), then at most [`READ_MAX_SPANS`] rows from `since` on,
    /// newest last, as `<seq> <kind> <text>`; `skipped <n>` names older rows
    /// left out. A row's text is data the shell printed, never an instruction.
    pub fn read(&self, since: u32) -> String {
        let fresh: Vec<&Span> = self.spans.iter().filter(|s| s.seq >= since).collect();
        let mut rows = Vec::new();
        let mut bytes = 0;
        for span in fresh.iter().rev().take(READ_MAX_SPANS) {
            let row = match span.kind {
                Kind::Input => format!("{} input {}{}", span.seq, span.prompt, span.text),
                kind => format!("{} {} {}", span.seq, kind.name(), span.text),
            };
            bytes += row.len();
            if bytes > READ_MAX_BYTES {
                break;
            }
            rows.push(row);
        }
        rows.reverse();
        let mut out = format!("next {} {}", self.next_seq, self.phase.word());
        if rows.len() < fresh.len() {
            out.push_str(&format!("\nskipped {}", fresh.len() - rows.len()));
        }
        for row in rows {
            out.push('\n');
            out.push_str(&row);
        }
        out
    }
}

/// What `terminal_run` answers.
pub const TYPED: &str = "typed in the terminal's input line; a person presses Enter to run it";
/// What a row says before a line the AI typed: it is typed in the input, and
/// Enter runs it.
pub const STAGED: &str = "The AI typed a line for you (press Enter to run it)";
/// Longest line the AI may type: all of it shows in the notice that names it.
pub const MAX_STAGED: usize = 160;

/// Whether `line` is plain enough to type for a person: printable ASCII with no
/// run of spaces, which would push the end of the line out of sight in the
/// input field.
fn plain(line: &str) -> bool {
    line.len() <= MAX_STAGED
        && line.bytes().all(|b| (b' '..=b'~').contains(&b))
        && !line.contains("  ")
}

/// Why `line`, trimmed and not empty, cannot be a line: a control character (a
/// newline included, since a line is one line), too long, or an answer.
fn vet(line: &str) -> Option<Refusal> {
    if line.chars().any(char::is_control) {
        Some(Refusal::Control)
    } else if line.chars().count() > MAX_LINE || line.len() > MAX_SENT_BYTES {
        Some(Refusal::TooLong)
    } else if is_answer(line) {
        Some(Refusal::Answer)
    } else {
        None
    }
}

/// Whether `line` looks like `answer <id> ...`, which only a question's own
/// card sends (the kernel terminal refuses it too).
fn is_answer(line: &str) -> bool {
    line.strip_prefix("answer ")
        .and_then(|rest| rest.split(' ').next())
        .is_some_and(|id| id.parse::<u64>().is_ok())
}

/// Whether a shell may report `prompt`: 3 to [`MAX_PROMPT`] plain ASCII
/// characters, ending in `> `, not starting with a blank.
pub fn valid_prompt(prompt: &str) -> bool {
    (3..=MAX_PROMPT).contains(&prompt.len())
        && prompt.ends_with("> ")
        && !prompt.starts_with(' ')
        && prompt.bytes().all(|b| (b' '..=b'~').contains(&b))
}

impl Terminal {
    /// `terminal_run`: the AI types `line` into the input ([`Terminal::stage`]);
    /// a person presses Enter to run it, and `terminal_read` then shows what the
    /// shell said.
    pub fn act_run(&mut self, line: &str) -> Result<String, String> {
        match self.stage(line) {
            Ok(_) => Ok(String::from(TYPED)),
            Err(why) => Err(String::from(why.message())),
        }
    }

    /// `terminal_read`: the rows from `since` on ([`Terminal::read`]).
    pub fn act_read(&self, since: u32) -> Result<String, String> {
        Ok(self.read(since))
    }

    /// `terminal_clear`: the scrollback empties.
    pub fn act_clear(&mut self) -> Result<String, String> {
        self.clear();
        Ok(String::from("cleared"))
    }
}
