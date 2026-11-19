//! The memory rows (ADR 0013 section 3; M2): what Kai keeps of the person's
//! words, in `kaimem`'s store, saved whole after each change (`Host::
//! memory_write`; the `memory` volume, M1, takes the file over).
//!
//! - **Remember.** The person's own "remember …" line keeps its own words
//!   (`kaimem::remember::from_line`), never the model's. A memory the model
//!   proposes asks on the card; the card is code-written (ADR 0005 §7) and the
//!   words are shown beside it, on `show`. One steered by content (a file, a
//!   page) is refused, whoever asks: read data never plants one. A fact kept
//!   already is said so, and nothing is logged or undone for it.
//! - **Edit and forget** run without a card only when the person's line asked
//!   for that very memory: the cue and its number, typed (`Why::Asked`). A
//!   "Taken as" from a vague line, a number the model chose, or anything else
//!   goes to the card, which `show` explains with the memory's words. The
//!   reply names what was changed or forgotten. Content never writes one.
//! - **Forget** is undone for 10 minutes; then the store purges it. **Forget
//!   all** always asks.
//! - **Agents** keep their own memories through their package (P3), never
//!   through these rows.
//! - **The log** keeps no memory's words: what was kept or forgotten is named
//!   by its number, and an edit's earlier words wait for `undo` in the shell
//!   (`memundo`), not on disk.

use crate::actions::Pending;
use crate::actlog::Change;
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
/// What a proposal the card cannot show as plain words is told.
pub const SAY_IT: &str =
    "that memory cannot be shown plainly beside the card; say \"remember ...\" yourself";

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

/// Whether the person's line asked for memory `memory` itself: the door
/// found the line asking (never a "Taken as"), and the number is typed in it.
fn line_names(request: &str, decision: Decision, memory: u64) -> bool {
    let number = format!("{memory}");
    decision.tier == Tier::Run
        && decision.why == Why::Asked
        && request
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == number)
}

impl Shell {
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
            Verdict::Asks { text, .. } if !text.is_ascii() => {
                Reply::err(format!("not remembered: {SAY_IT}"))
            }
            Verdict::Asks { text, .. } => {
                if let Some(kept) = self.memory_kept(&text) {
                    return Reply::ok(format!("Kai already remembers that (memory {kept})"));
                }
                let call =
                    call_text("memory_remember", &[CallArg::Int(id as i64)]).unwrap_or_default();
                let length = text.chars().count();
                let question = format!(
                    "Remember a fact of {length} characters with {call}? Type show to read it"
                );
                let note = format!("Kai keeps \"{text}\" for every request until you forget it");
                self.memory_ask(id, request, &call, note, &question, Waiting::Remember(text))
            }
        }
    }

    /// The memory already kept for everyone with `text`, if any.
    fn memory_kept(&self, text: &str) -> Option<u64> {
        let text = text.trim();
        self.memories
            .list(Who::User)
            .iter()
            .find(|m| m.scope == Scope::All && m.text.eq_ignore_ascii_case(text))
            .map(|m| m.id)
    }

    /// Keeps `text` for the person, logged with its forget as the undo. A
    /// fact kept already changes nothing, so nothing is logged to undo.
    fn memory_keep(&mut self, id: u64, request: &str, text: &str, consent: &str) -> Reply {
        if let Some(kept) = self.memory_kept(text) {
            return Reply::ok(format!("Kai already remembers that (memory {kept})"));
        }
        let now = self.host.now_ns();
        let kept = self.memories.remember(Who::User, text, Scope::All, id, now);
        let memory = match kept {
            Ok(memory) => memory,
            Err(why) => return Reply::err(format!("not remembered: {why}")),
        };
        let change = memory_change("memory_remember", memory, "", "kept");
        let change = Change {
            inverse: Some(Inverse::MemoryForget { id: memory }),
            ..change
        };
        self.memory_logged(id, request, "memory_remember", consent, alloc::vec![change]);
        Reply::ok(format!("Kai remembers \"{text}\" (memory {memory}). undo"))
    }

    /// `memory_edit`: the person's line naming the memory and its new words
    /// runs; anything else asks on the card.
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
        if let Some(why) = kaimem::record::text_problem(text.trim()) {
            return Reply::err(format!("not changed: {why}"));
        }
        let Some(old) = self.memories.get(Who::User, memory).map(|m| m.text.clone()) else {
            return Reply::err(format!("no memory {memory}; memory_list shows them"));
        };
        if line_names(request, decision, memory) {
            return self.memory_edit_run(id, request, memory, text, "asked");
        }
        if !text.is_ascii() {
            return Reply::err(format!("not changed: {SAY_IT}"));
        }
        let call = call_text("memory_edit", &[CallArg::Int(memory as i64)]).unwrap_or_default();
        let question =
            format!("Change memory {memory} with {call}? Type show to read the new words");
        let note = format!(
            "memory {memory} says \"{old}\"; it would say \"{}\"",
            text.trim()
        );
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
        let after = String::from(text.trim());
        self.memory_edited(memory, &after, &before);
        let change = Change {
            inverse: Some(Inverse::MemoryText { id: memory }),
            ..memory_change("memory_edit", memory, "words before", "edited")
        };
        self.memory_logged(id, request, "memory_edit", consent, alloc::vec![change]);
        Reply::ok(format!(
            "Done: memory {memory} said \"{before}\" and says \"{after}\" now. undo"
        ))
    }

    /// `memory_forget`: runs when the person's line named that memory, else
    /// asks.
    fn memory_forget(&mut self, id: u64, request: &str, memory: u64, decision: Decision) -> Reply {
        let Some(text) = self.memories.get(Who::User, memory).map(|m| m.text.clone()) else {
            return Reply::err(format!("no memory {memory}; memory_list shows them"));
        };
        if line_names(request, decision, memory) {
            return self.memory_forget_run(id, request, &[memory], "asked");
        }
        let call = call_text("memory_forget", &[CallArg::Int(memory as i64)]).unwrap_or_default();
        let question = format!("Forget memory {memory} with {call}? Type show to read it");
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

    /// Forgets each of `memories`, logged with their restore as the undo;
    /// the reply names what went.
    fn memory_forget_run(
        &mut self,
        id: u64,
        request: &str,
        memories: &[u64],
        consent: &str,
    ) -> Reply {
        let now = self.host.now_ns();
        let mut changes = Vec::new();
        let mut gone = Vec::new();
        for &memory in memories {
            let Some(text) = self.memories.get(Who::User, memory).map(|m| m.text.clone()) else {
                continue;
            };
            if self.memories.forget(Who::User, memory, now).is_ok() {
                changes.push(Change {
                    inverse: Some(Inverse::MemoryRestore { id: memory }),
                    ..memory_change("memory_forget", memory, "kept", "forgotten")
                });
                gone.push((memory, text));
            }
        }
        if changes.is_empty() {
            return Reply::err("nothing was forgotten");
        }
        self.memory_logged(id, request, "memory_forget", consent, changes);
        match gone.as_slice() {
            [(memory, text)] => Reply::ok(format!(
                "Forgotten: memory {memory}, \"{text}\". undo for 10 minutes"
            )),
            all => Reply::ok(format!(
                "Forgotten: {} memories. undo for 10 minutes",
                all.len()
            )),
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
}

/// A memory change for the log, which keeps no memory's words: `before` and
/// `after` say what happened to memory `memory`.
fn memory_change(action: &str, memory: u64, before: &str, after: &str) -> Change {
    Change {
        action: String::from(action),
        owner: String::from("memory"),
        res: format!("memory {memory}"),
        before: String::from(before),
        after: String::from(after),
        inverse: None,
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
