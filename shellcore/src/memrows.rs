//! The memory rows (ADR 0013 section 3; M2): what Kai keeps of the person's
//! words, in `kaimem`'s store, saved whole after each change (`Host::
//! memory_write`; the `memory` volume, M1, takes the file over).
//!
//! - **Remember.** The person's own "remember …" line keeps its own words
//!   (`kaimem::remember::from_line`), never the model's. A memory the model
//!   proposes asks on the card with its exact text. One steered by content (a
//!   file, a page) is refused, whoever asks: read data never plants one.
//! - **Edit** keeps a text the line wrote, and asks for any other; content
//!   never writes one.
//! - **Forget** runs and `undo` brings it back for 10 minutes; then the store
//!   purges it. **Forget all** always asks.
//! - **Agents** keep their own memories through their package (P3), never
//!   through these rows.
//! - **The State** carries at most 3 memories that share a word with the
//!   request, inside its 64 tokens ([`Shell::state_line_for`]). They inform
//!   the model and never give consent.

use crate::actions::Pending;
use crate::actlog::{Change, Entry, Form, State};
use crate::undo::Inverse;
use crate::{Reply, Shell};
use agentabi::Tier;
use agentabi::consent::{Decision, Label, Why};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kaimem::record::Scope;
use kaimem::remember::{Asker, Verdict};
use kaimem::store::Who;
use termcore::once::{CallArg, call_text};

/// What an agent's step to a memory row is told.
pub const AGENTS_OWN: &str = "an agent keeps its own memories through its package, not these rows";

/// What a memory row's ask waits on.
#[derive(Clone, Debug)]
pub(crate) enum Waiting {
    Remember(String),
    Edit(u64, String),
    Forget(u64),
    ForgetAll,
}

/// `name`'s argument `arg`, as text.
fn value<'a>(values: &'a [(&'static str, String)], arg: &str) -> Option<&'a str> {
    values
        .iter()
        .find(|(n, _)| *n == arg)
        .map(|(_, v)| v.as_str())
}

/// Content that steered the call, by its source, if any did.
fn content(why: Why) -> Option<&'static str> {
    match why {
        Why::Steered(source) | Why::Outbound(source) | Why::GrantSteered(source) => Some(source),
        _ => None,
    }
}

impl Shell {
    /// The memories `memory_read` holds, loaded at start; records out of
    /// shape are skipped and said.
    pub(crate) fn with_saved_memories(mut self) -> Self {
        let Some(text) = self.host.memory_read() else {
            return self;
        };
        match kaimem::store::Memories::load(&text) {
            Ok(loaded) => {
                for skipped in &loaded.skipped {
                    self.host
                        .audit(&format!("a memory record was skipped: {skipped:?}"));
                }
                self.memories = loaded.memories;
            }
            Err(why) => self.host.audit(&format!(
                "the memory file is out of shape, so none is read: {why}"
            )),
        }
        self
    }

    /// A memory row's call, which `call_row` hands here with its tier decided.
    pub(crate) fn memory_call(
        &mut self,
        id: u64,
        request: &str,
        row: &str,
        values: &[(&'static str, String)],
        decision: Decision,
    ) -> Reply {
        if self.agent_acting().is_some() {
            return Reply::err(AGENTS_OWN);
        }
        self.memory_purge();
        let memory = value(values, "id").and_then(|v| v.parse::<u64>().ok());
        let text = value(values, "text").unwrap_or_default();
        match (row, memory) {
            ("memory_list", _) => Reply::ok(listed(&self.memories.list(Who::User))),
            ("memory_search", _) => {
                let query = value(values, "query").unwrap_or_default();
                Reply::ok(listed(&self.memories.search(Who::User, query)))
            }
            ("memory_remember", _) => self.memory_remember(id, request, text, decision),
            ("memory_edit", Some(memory)) => self.memory_edit(id, request, memory, text, decision),
            ("memory_forget", Some(memory)) => self.memory_forget(id, request, memory, decision),
            ("memory_forget_all", _) => self.memory_forget_all_ask(id, request),
            _ => Reply::err(format!("{row} needs its id")),
        }
    }

    /// `memory_remember`: the line's own words run; the model's ask.
    fn memory_remember(&mut self, id: u64, request: &str, text: &str, decision: Decision) -> Reply {
        let label = match content(decision.why) {
            Some(source) => Label::Content(source),
            None => Label::Literal,
        };
        let asker = match (label, kaimem::remember::from_line(request)) {
            (Label::Literal, Some(_)) => Asker::Line(request),
            _ => Asker::Proposal { text, label },
        };
        match kaimem::remember::decide(asker, true) {
            Verdict::Refused(why) => Reply::err(format!("not remembered: {why}")),
            Verdict::Runs { text, .. } => self.memory_keep(id, request, &text, "asked"),
            Verdict::Asks { text, .. } => {
                let call =
                    call_text("memory_remember", &[CallArg::Int(id as i64)]).unwrap_or_default();
                let question = format!("Remember \"{text}\" with {call}?");
                let note = format!("Kai keeps \"{text}\" for every request until you forget it");
                self.memory_ask(id, request, &call, note, &question, Waiting::Remember(text))
            }
        }
    }

    /// Keeps `text` for the person, logged with its forget as the undo.
    fn memory_keep(&mut self, id: u64, request: &str, text: &str, consent: &str) -> Reply {
        let now = self.host.now_ns();
        let kept = self.memories.remember(Who::User, text, Scope::All, id, now);
        let memory = match kept {
            Ok(memory) => memory,
            Err(why) => return Reply::err(format!("not remembered: {why}")),
        };
        let change = Change {
            action: String::from("memory_remember"),
            owner: String::from("memory"),
            res: format!("memory {memory}"),
            before: String::new(),
            after: String::from(text),
            inverse: Some(Inverse::MemoryForget { id: memory }),
        };
        self.memory_logged(id, request, "memory_remember", consent, alloc::vec![change]);
        Reply::ok(format!("Kai remembers \"{text}\" (memory {memory}). undo"))
    }

    /// `memory_edit`: a text the line wrote runs; any other asks.
    fn memory_edit(
        &mut self,
        id: u64,
        request: &str,
        memory: u64,
        text: &str,
        decision: Decision,
    ) -> Reply {
        if let Some(source) = content(decision.why) {
            return Reply::err(format!(
                "not changed: a memory is never written from content ({source})"
            ));
        }
        if self.memories.get(Who::User, memory).is_none() {
            return Reply::err(format!("no memory {memory}; memory_list shows them"));
        }
        if decision.tier == Tier::Run {
            return self.memory_edit_run(id, request, memory, text, "asked");
        }
        let call = call_text("memory_edit", &[CallArg::Int(memory as i64)]).unwrap_or_default();
        let question = format!("Change memory {memory} to \"{text}\" with {call}?");
        let note = format!("memory {memory} will say \"{text}\"");
        let waiting = Waiting::Edit(memory, String::from(text));
        self.memory_ask(id, request, &call, note, &question, waiting)
    }

    fn memory_edit_run(
        &mut self,
        id: u64,
        request: &str,
        memory: u64,
        text: &str,
        consent: &str,
    ) -> Reply {
        let now = self.host.now_ns();
        let before = match self.memories.edit(Who::User, memory, text, now) {
            Ok(before) => before,
            Err(why) => return Reply::err(format!("not changed: {why}")),
        };
        let change = Change {
            action: String::from("memory_edit"),
            owner: String::from("memory"),
            res: format!("memory {memory}"),
            before,
            after: String::from(text.trim()),
            inverse: Some(Inverse::MemoryText { id: memory }),
        };
        self.memory_logged(id, request, "memory_edit", consent, alloc::vec![change]);
        Reply::ok(format!(
            "Done: memory {memory} says \"{}\". undo",
            text.trim()
        ))
    }

    /// `memory_forget`: runs when the line asked, else asks.
    fn memory_forget(&mut self, id: u64, request: &str, memory: u64, decision: Decision) -> Reply {
        let Some(text) = self.memories.get(Who::User, memory).map(|m| m.text.clone()) else {
            return Reply::err(format!("no memory {memory}; memory_list shows them"));
        };
        if decision.tier == Tier::Run {
            return self.memory_forget_run(id, request, &[memory], "asked");
        }
        let call = call_text("memory_forget", &[CallArg::Int(memory as i64)]).unwrap_or_default();
        let question = format!("Forget memory {memory} with {call}?");
        let note = format!("Kai forgets \"{text}\"; undo brings it back for 10 minutes");
        self.memory_ask(id, request, &call, note, &question, Waiting::Forget(memory))
    }

    /// `memory_forget_all`: always asks, with how many.
    fn memory_forget_all_ask(&mut self, id: u64, request: &str) -> Reply {
        let count = self.memories.list(Who::User).len();
        if count == 0 {
            return Reply::ok("Kai remembers nothing yet");
        }
        let call = String::from("memory_forget_all()");
        let question = format!(
            "Forget all {count} memories with {call}? Undo brings them back for 10 minutes"
        );
        let note =
            format!("Kai forgets all {count} memories; undo brings them back for 10 minutes");
        self.memory_ask(id, request, &call, note, &question, Waiting::ForgetAll)
    }

    /// Forgets each of `memories`, logged with their restore as the undo.
    fn memory_forget_run(
        &mut self,
        id: u64,
        request: &str,
        memories: &[u64],
        consent: &str,
    ) -> Reply {
        let now = self.host.now_ns();
        let mut changes = Vec::new();
        for &memory in memories {
            let Some(text) = self.memories.get(Who::User, memory).map(|m| m.text.clone()) else {
                continue;
            };
            if self.memories.forget(Who::User, memory, now).is_ok() {
                changes.push(Change {
                    action: String::from("memory_forget"),
                    owner: String::from("memory"),
                    res: format!("memory {memory}"),
                    before: text,
                    after: String::from("forgotten"),
                    inverse: Some(Inverse::MemoryRestore { id: memory }),
                });
            }
        }
        if changes.is_empty() {
            return Reply::err("nothing was forgotten");
        }
        let count = changes.len();
        self.memory_logged(id, request, "memory_forget", consent, changes);
        match count {
            1 => Reply::ok(format!(
                "Forgotten: memory {}. undo for 10 minutes",
                memories[0]
            )),
            n => Reply::ok(format!("Forgotten: {n} memories. undo for 10 minutes")),
        }
    }

    fn memory_ask(
        &mut self,
        id: u64,
        request: &str,
        call: &str,
        note: String,
        question: &str,
        waiting: Waiting,
    ) -> Reply {
        match self.card_ask(id, request, call, note, question) {
            Ok(reply) => {
                if let Some(pending) = self.pending.as_mut() {
                    pending.memory = Some(waiting);
                }
                reply
            }
            Err(refused) => refused,
        }
    }

    /// The answer to a memory row's ask: an Allow whose ONCE checks runs it.
    pub(crate) fn answer_memory(&mut self, pending: Pending, text: &str) -> Reply {
        let Some(waiting) = pending.memory.clone() else {
            return Reply::err("nothing to confirm");
        };
        if let Err(reply) = self.allowed(&pending, text) {
            return reply;
        }
        let (id, request) = (pending.id, pending.request.clone());
        let reply = match waiting {
            Waiting::Remember(text) => self.memory_keep(id, &request, &text, "yes"),
            Waiting::Edit(memory, text) => self.memory_edit_run(id, &request, memory, &text, "yes"),
            Waiting::Forget(memory) => self.memory_forget_run(id, &request, &[memory], "yes"),
            Waiting::ForgetAll => {
                let all: Vec<u64> = self.memories.list(Who::User).iter().map(|m| m.id).collect();
                self.memory_forget_run(id, &request, &all, "yes")
            }
        };
        self.remember(&pending, &reply.text);
        reply
    }

    fn memory_logged(
        &mut self,
        id: u64,
        request: &str,
        row: &str,
        consent: &str,
        changes: Vec<Change>,
    ) {
        let script = changes
            .first()
            .map(|c| format!("{row}({})", c.res.trim_start_matches("memory ")))
            .unwrap_or_default();
        self.log.push(Entry {
            agent: agentid::Name::kai(),
            id,
            t_ns: self.host.now_ns(),
            form: Form::Call,
            of: None,
            tier: Some(if consent == "yes" {
                Tier::Ask
            } else {
                Tier::Run
            }),
            pack: None,
            consent: String::from(consent),
            state: State::Done,
            request: String::from(request),
            script,
            outcome: String::from("ran"),
            changes,
        });
        self.save_log();
        self.memory_save();
    }

    /// Drops memories forgotten more than 10 minutes ago, for good.
    fn memory_purge(&mut self) {
        if self.memories.purge(self.host.now_ns()) > 0 {
            self.memory_save();
        }
    }

    fn memory_save(&mut self) {
        if let Err(why) = self.host.memory_write(&self.memories.save()) {
            self.host
                .audit(&format!("the memories were not saved: {why}"));
        }
    }

    /// `undo` of a memory change: `MemoryRestore` brings a forgotten one
    /// back (within 10 minutes), `MemoryForget` forgets one again, and
    /// `MemoryText` sets one back to `text`.
    pub(crate) fn revert_memory(&mut self, inverse: Inverse, text: &str) -> Result<String, String> {
        let now = self.host.now_ns();
        let done = match inverse {
            Inverse::MemoryRestore { id } => self
                .memories
                .restore(Who::User, id, now)
                .map(|()| format!("memory {id} is back")),
            Inverse::MemoryForget { id } => self
                .memories
                .forget(Who::User, id, now)
                .map(|()| format!("memory {id} forgotten")),
            Inverse::MemoryText { id } => self
                .memories
                .edit(Who::User, id, text, now)
                .map(|_| format!("memory {id} says \"{text}\" again")),
            _ => return Err(String::from("not a memory change")),
        };
        if done.is_ok() {
            self.memory_save();
        }
        done
    }

    /// The State the next request `request` carries: the OS's, with the
    /// memories that share a word with it in the room the line has left.
    pub(crate) fn state_line_for(&self, request: &str) -> Option<String> {
        let state = self.host.state();
        let memories = self.memories.list(Who::User);
        let memory = kaimem::prompt::state(&memories, request, state.memory_room());
        state.snapshot_with(&memory)
    }
}

/// Memories as `memory_list` shows them, a line each.
fn listed(memories: &[&kaimem::record::Memory]) -> String {
    if memories.is_empty() {
        return String::from("Kai remembers nothing yet");
    }
    let lines: Vec<String> = memories
        .iter()
        .map(|m| format!("memory {}: {}", m.id, m.text))
        .collect();
    lines.join("\n")
}
