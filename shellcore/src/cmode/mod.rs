//! The `c>` prompt in the shell (ADR 0012 D1, D4, D5; P4 task 12): `:c`
//! enters C mode and `:rhai` leaves it; `:c <entry>` runs one entry from
//! `kai>`. Each entry goes to the `cc` service (`kai-cc 1`, promptcore's wire).
//! When it calls a `kai` import, cc stops at a `need`, the call goes through
//! the one door as part of the entry ([`Shell::entry_call`]), and its value
//! goes back with `resume`. A call that asks stops the entry at the user's
//! prompt until they answer.
//!
//! - **Only `kai` imports**: a `need` names one of the rows the one door runs
//!   (the bound, file and package rows), or it is refused. cc runs untrusted
//!   code, so the shell's own words (`undo`, `script`, `say`) never come from
//!   it.
//! - **Answers come from the prompt**: an entry waiting on a question goes
//!   on only for `answer <its id> …`, which the terminal sends from that
//!   question's own prompt; anything else drops the entry.
//! - **Bounded**: at most [`MAX_CALLS`] calls per entry, and Ctrl-C between
//!   them.
//! - **One entry, one log entry**: the calls of an entry join one actlog
//!   entry ([`Shell::entry_end`]); a failed entry is put back at once. A
//!   content read that asked labels the rest of the entry as one that ran
//!   at once does (ADR 0002 D6).
//! - `undo`, `:dis`, `:defs`, `:reset` and a restarted cc: `session.rs`.

mod session;

use crate::{CANCEL_ENTRY, Reply, Shell};
use agentabi::{RetType, Returns};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use promptcore::wire::{Record, Request, parse_reply};
use session::{Ran, no_session, resume};

/// Most `kai` calls one entry may make: each goes through the one door and
/// into the log.
pub const MAX_CALLS: u32 = 256;

/// An entry stopped at the user's prompt by a call that asks.
#[derive(Clone, Debug)]
struct Waiting {
    seq: u64,
    text: String,
    /// The `need` record's id.
    need: u64,
    /// The asked call's id, for [`Shell::take_value`], and its row.
    ask: u64,
    row: String,
    /// What the entry printed before it stopped.
    shown: String,
}

/// The shell's side of `c>`.
#[derive(Clone, Debug, Default)]
pub(crate) struct CMode {
    /// At `c>` rather than `kai>`.
    on: bool,
    /// cc has this shell's session open.
    open: bool,
    seq: u64,
    ran: Vec<Ran>,
    waiting: Option<Waiting>,
    /// An entry with brackets still open.
    partial: Option<String>,
    /// `kai` calls the running entry has made.
    calls: u32,
}

/// What the shell says on `:c`.
const ENTERED: &str = "C mode: each entry compiles and runs on Enter; :rhai goes back";

impl Shell {
    /// Whether the shell is at `c>`.
    pub fn at_c(&self) -> bool {
        self.c.on
    }

    /// The line at `c>`, or a `:c` line at `kai>`; `None` for every other line
    /// at `kai>`.
    pub(crate) fn c_line(&mut self, input: &str) -> Option<Reply> {
        if let Some(entry) = self.c.partial.take() {
            return Some(self.c_continue(entry, input));
        }
        let line = input.trim();
        if self.c.waiting.is_some() {
            if let Some(reply) = self.c_answer(line) {
                return Some(reply);
            }
            self.c_abandon("another line came first");
        }
        if !self.c.on {
            return match line.strip_prefix(":c") {
                Some("") => {
                    self.c.on = true;
                    Some(Reply::ok(ENTERED))
                }
                Some(rest) if rest.starts_with(' ') => Some(self.c_entry(rest.trim())),
                _ if line == ":rhai" => Some(Reply::ok("already at kai>")),
                _ => None,
            };
        }
        Some(match line {
            ":rhai" => {
                self.c.on = false;
                Reply::ok("back at kai>")
            }
            ":c" => Reply::ok("already in C mode"),
            "" => Reply::ok(""),
            "undo" => self.c_undo(),
            ":defs" => self.c_command(&Request::Defs),
            ":reset" => self.c_reset(),
            _ => match line.strip_prefix(":dis ") {
                Some(name) => self.c_command(&Request::Dis(name.trim())),
                None if line.starts_with(':') => {
                    Reply::err("at c> the commands are :dis f, :defs, :reset, undo and :rhai")
                }
                None => self.c_entry(input.trim_end()),
            },
        })
    }

    fn c_continue(&mut self, mut entry: String, input: &str) -> Reply {
        if input == CANCEL_ENTRY {
            return Reply::ok("");
        }
        entry.push('\n');
        entry.push_str(input.trim_end());
        self.c_entry(&entry)
    }

    /// Runs one entry, or keeps it for the next line while brackets are open.
    fn c_entry(&mut self, text: &str) -> Reply {
        if text.len() > promptcore::MAX_ENTRY {
            return Reply::err(format!(
                "dropped: an entry may be at most {} KiB",
                promptcore::MAX_ENTRY / 1024
            ));
        }
        if promptcore::open(text) {
            self.c.partial = Some(text.to_string());
            return Reply::more();
        }
        let mut restored = match self.c_ensure_open() {
            Ok(note) => note,
            Err(why) => return Reply::err(why),
        };
        self.c.seq += 1;
        self.c.calls = 0;
        let seq = self.c.seq;
        self.entry_begin(text);
        let request = Request::Eval { seq, entry: text }.text();
        let mut reply = self.host.cc(request.as_bytes());
        // A cc the supervisor restarted between entries has no session: open
        // it again, replay, and send the entry once more. Nothing ran.
        if reply.as_deref().is_ok_and(no_session) {
            self.c.open = false;
            restored = match self.c_ensure_open() {
                Ok(note) => note,
                Err(why) => return self.c_failed("", &why),
            };
            reply = self.host.cc(request.as_bytes());
        }
        match reply {
            Ok(reply) => self.c_drive(seq, text, reply, restored),
            Err(why) => {
                self.c.open = false;
                self.c_failed(&restored, &format!("cc did not answer: {why}"))
            }
        }
    }

    /// Follows cc's replies for entry `seq` until it ends or a call asks.
    fn c_drive(&mut self, seq: u64, text: &str, mut reply: Vec<u8>, mut shown: String) -> Reply {
        loop {
            let records = match parse_reply(&reply) {
                Ok(records) => records,
                Err(e) => return self.c_failed(&shown, &format!("cc's reply: {e}")),
            };
            let mut defined = false;
            let mut value = String::new();
            let mut next = None;
            for record in &records {
                match *record {
                    Record::Out(bytes) => shown.push_str(&String::from_utf8_lossy(bytes)),
                    Record::Def { .. } => defined = true,
                    Record::Retyped(name) => shown.push_str(&format!(
                        "{name} changed type; code that already called it keeps the old one\n"
                    )),
                    Record::Value { ty, text } => {
                        let text = String::from_utf8_lossy(text);
                        value = match ty {
                            promptcore::wire::ValueType::Str => format!("= \"{text}\""),
                            _ => format!("= {text}"),
                        };
                    }
                    Record::Need { id, call } => next = Some((id, call.to_vec())),
                    Record::Done { .. } => {
                        let logged = self.entry_end(true);
                        let defined = defined.then(|| text.to_string());
                        self.c.ran.push(Ran {
                            seq,
                            defined,
                            logged,
                        });
                        if !value.is_empty() && !shown.is_empty() && !shown.ends_with('\n') {
                            shown.push('\n');
                        }
                        shown.push_str(&value);
                        return Reply::ok(shown.trim_end_matches('\n'));
                    }
                    Record::Err { text: why, .. } => {
                        return self.c_failed(&shown, &String::from_utf8_lossy(why));
                    }
                    Record::Trap { reason, function } => {
                        let at = function.map(|f| format!(" in {f}")).unwrap_or_default();
                        return self.c_failed(&shown, &format!("stopped: {reason}{at}"));
                    }
                    Record::Undone(_) | Record::Text(_) | Record::Ok => {}
                }
            }
            let Some((need, call)) = next else {
                return self.c_failed(&shown, "cc's reply did not end the entry");
            };
            let resume = match self.c_call(seq, text, need, &call, &shown) {
                Ok(resume) => resume,
                Err(stop) => return stop,
            };
            reply = match self.host.cc(resume.as_bytes()) {
                Ok(reply) => reply,
                Err(why) => {
                    self.c.open = false;
                    return self.c_failed(&shown, &format!("cc stopped: {why}"));
                }
            };
        }
    }

    /// One `kai` call through the one door: the `resume` to send, or the reply
    /// that stops here: the question of a call that asks (the entry waits), or
    /// why the entry ended.
    fn c_call(
        &mut self,
        seq: u64,
        text: &str,
        need: u64,
        call: &[u8],
        shown: &str,
    ) -> Result<String, Reply> {
        self.c.calls += 1;
        if self.c.calls > MAX_CALLS {
            let why = format!("stopped: more than {MAX_CALLS} kai calls in one entry");
            return Err(self.c_failed(shown, &why));
        }
        if self.host.interrupted() {
            return Err(self.c_failed(shown, "stopped: interrupted"));
        }
        let call = core::str::from_utf8(call)
            .ok()
            .and_then(|json| llmproto::act::parse(json).ok());
        let Some(call) = call else {
            return Ok(resume(need, Err(String::from("cc sent a malformed call"))));
        };
        if !kai_row(&call.name) {
            let why = format!("refused: {} is not a kai call", call.name);
            return Ok(resume(need, Err(why)));
        }
        let outcome = self.entry_call(&call);
        match (outcome.value, outcome.reply.confirm) {
            (Some(value), _) => Ok(resume(need, value)),
            (None, Some(ask)) => {
                self.c.waiting = Some(Waiting {
                    seq,
                    text: text.to_string(),
                    need,
                    ask,
                    row: call.name,
                    shown: shown.to_string(),
                });
                Err(outcome.reply)
            }
            (None, None) => Ok(resume(need, Err(outcome.reply.text))),
        }
    }

    /// The answer to the call that asked, if `line` is `answer <its id> …`:
    /// the entry goes on with its value. `show` and `why` keep it waiting.
    fn c_answer(&mut self, line: &str) -> Option<Reply> {
        let ask = self.c.waiting.as_ref()?.ask;
        let rest = line.strip_prefix("answer ")?;
        let id = rest.split(' ').next()?.parse::<u64>().ok()?;
        if id != ask {
            return None;
        }
        let answered = self.answer(rest);
        if answered.confirm.is_some() {
            return Some(answered);
        }
        let waiting = self.c.waiting.take()?;
        let value = self
            .take_value(waiting.ask)
            .unwrap_or_else(|| Err(String::from("refused: not answered")));
        // Content an asked call read labels the rest of the entry too.
        if value.is_ok()
            && let Some(source) = content_source(&waiting.row)
            && let Some(entry) = self.c_entry.as_mut()
        {
            entry.seen.get_or_insert(source);
        }
        let mut shown = waiting.shown;
        if !answered.text.is_empty() {
            shown.push_str(&answered.text);
            shown.push('\n');
        }
        let request = resume(waiting.need, value);
        Some(match self.host.cc(request.as_bytes()) {
            Ok(reply) => self.c_drive(waiting.seq, &waiting.text, reply, shown),
            Err(why) => {
                self.c.open = false;
                self.c_failed(&shown, &format!("cc stopped: {why}"))
            }
        })
    }

    /// Drops an entry that waits on an answer: cc drops its run on our next
    /// request, its question is discarded, and its changes so far are put
    /// back.
    fn c_abandon(&mut self, why: &str) {
        if let Some(waiting) = self.c.waiting.take() {
            self.value_rets.retain(|(id, _)| *id != waiting.ask);
        }
        if let Some(pending) = self.pending.take() {
            let outcome = format!("discarded: {why}");
            self.record(
                pending.id,
                &pending.request,
                &pending.checked.script,
                &outcome,
            );
        }
        self.entry_end(false);
    }

    fn c_failed(&mut self, shown: &str, why: &str) -> Reply {
        if let Some(waiting) = self.c.waiting.take() {
            self.value_rets.retain(|(id, _)| *id != waiting.ask);
        }
        self.entry_end(false);
        let mut text = String::from(shown);
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(why.trim_end());
        Reply::err(text)
    }
}

/// Whether `name` is a row the one door runs as a call: what a `kai` import
/// may be (ccserve's `Rows::os`).
fn kai_row(name: &str) -> bool {
    [
        &agentabi::BOUND[..],
        &agentabi::FS_ROWS,
        &agentabi::PKG_ROWS,
    ]
    .iter()
    .any(|rows| rows.iter().any(|row| row.name == name))
}

/// The source of the content row `name` returns, if it returns content.
fn content_source(name: &str) -> Option<&'static str> {
    let row = agentabi::find(&agentabi::ROWS, name)?;
    match (row.returns, row.ret) {
        (Returns::Content(source), _) => Some(source),
        (_, RetType::Data) => Some("data"),
        _ => None,
    }
}
