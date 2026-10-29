//! A C entry at the `c>` prompt (P4 task 12, ADR 0012 D5/D6): the program's
//! `kai` calls go through the one door, each decided against the entry the
//! user typed, and come back as typed values for `ccserve` to hand the C code.
//!
//! - One entry, one log entry: every call's changes join one actlog entry when
//!   the entry ends, so `undo` puts the whole entry back, newest change first;
//!   an entry that failed is put back at once.
//! - Data labels: once a call returns content (`Data`), every later call of
//!   the entry is decided as content's (ADR 0002 D6), so a change asks.
//! - Values: a row's result as JSON by its `RetType`: a number, a string (text
//!   and `Data`), an object, an array, or `null`. A call refused by its tier or
//!   by consent answers `refused: <why>`, a declined ask `refused: declined`,
//!   a run that failed its error.
//! - Asks: a call that asks leaves no value yet; once the user answers at the
//!   prompt, [`Shell::take_value`] has it.
//!
//! Calls of an entry run in the shell, never in a `scriptrun` child, so their
//! values come back.

use crate::Shell;
use crate::actlog::{Entry, Form, State};
use crate::agentcore::Outcome;
use crate::tiers::Tier;
use agentabi::{RetType, Returns};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use llmproto::act::Call;
use rhai::Dynamic;
use serde_json::{Map, Number, Value};

/// The C entry now running.
pub(crate) struct CEntry {
    /// The last log entry before it began: its calls' entries come after.
    start: u64,
    request: String,
    /// The source of the first content a call returned, if any did.
    pub(crate) seen: Option<&'static str>,
}

/// What a call of an entry came to, for `ccserve`'s `resume`.
pub type CallValue = Result<Value, String>;

impl Shell {
    /// Begins C entry `request`, as the user typed it: its token.
    pub fn entry_begin(&mut self, request: &str) -> u64 {
        let start = self.log.last_id().max(self.next_id.saturating_sub(1));
        self.c_entry = Some(CEntry {
            start,
            request: request.to_string(),
            seen: None,
        });
        start
    }

    /// One `kai` call of the running entry: what it came to, with its value
    /// unless it asks (then [`Shell::take_value`] has it once answered).
    pub fn entry_call(&mut self, call: &Call) -> Outcome {
        let Some(request) = self.c_entry.as_ref().map(|e| e.request.clone()) else {
            return crate::agentcore::outcome_refused("no C entry is running");
        };
        let id = self.next_id;
        self.next_id += 1;
        self.last_value = None;
        let row = self.row_of(&call.name);
        let mut outcome = self.execute(id, &request, call);
        let value = match (outcome.tier, outcome.reply.confirm, row) {
            (_, Some(asked), Some(row)) => {
                self.value_rets.push((asked, row.ret));
                None
            }
            (Tier::Never, _, _) => Some(Err(refused(&outcome.reply.text))),
            (_, _, Some(row)) if outcome.reply.ok => {
                let value = self.last_value.take();
                Some(Ok(json_of(value.as_ref(), &row.ret)))
            }
            (_, _, _) => Some(Err(outcome.reply.text.clone())),
        };
        // A call that read content makes the rest of the entry content's.
        if let (Some(row), Some(Ok(_))) = (row, &value)
            && let Some(source) = content_source(row)
            && let Some(entry) = self.c_entry.as_mut()
        {
            entry.seen.get_or_insert(source);
        }
        outcome.value = value;
        outcome
    }

    /// Ends the running entry: its calls' log entries become one, which an
    /// entry that failed (`ok` false) puts back at once. Its id, if it changed
    /// anything; an entry that only read is logged too.
    pub fn entry_end(&mut self, ok: bool) -> Option<u64> {
        let entry = self.c_entry.take()?;
        let parts = self.log.take_entries_after(entry.start);
        if parts.is_empty() {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        let changes: Vec<_> = parts.iter().flat_map(|p| p.changes.clone()).collect();
        let script: Vec<&str> = parts.iter().map(|p| p.script.as_str()).collect();
        let outcome: Vec<&str> = parts.iter().map(|p| p.outcome.as_str()).collect();
        let tier = parts.iter().filter_map(|p| p.tier).max();
        let state = match changes.is_empty() {
            true => State::Nothing,
            false => State::Done,
        };
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: Form::Call,
            of: None,
            tier,
            pack: None,
            consent: String::from("c-entry"),
            state,
            request: entry.request,
            script: script.join("\n"),
            outcome: outcome.join("; "),
            changes,
        });
        if !ok && state == State::Done {
            let reply = self.revert_entry(id, false);
            self.host.audit(&format!(
                "C entry {id} failed and was put back: {}",
                reply.text
            ));
        }
        self.save_log();
        (state == State::Done).then_some(id)
    }

    /// The value of asked call `id`, once its answer ran or declined it.
    pub fn take_value(&mut self, id: u64) -> Option<CallValue> {
        let at = self.values.iter().position(|(v, _)| *v == id)?;
        Some(self.values.remove(at).1)
    }

    /// Called as an asked call `id` runs (`ran` its value, or why it failed)
    /// or is declined (`None`): keeps its value for [`Shell::take_value`].
    pub(crate) fn asked_done(&mut self, id: u64, ran: Option<Result<Option<Dynamic>, String>>) {
        let Some(at) = self.value_rets.iter().position(|(v, _)| *v == id) else {
            return;
        };
        let (_, ret) = self.value_rets.remove(at);
        let value = match ran {
            None => Err(String::from("refused: declined")),
            Some(Ok(value)) => Ok(json_of(value.as_ref(), &ret)),
            Some(Err(why)) => Err(why),
        };
        self.values.push((id, value));
    }

    fn row_of(&self, name: &str) -> Option<&'static agentabi::Action> {
        agentabi::find(&agentabi::ROWS, name)
    }
}

/// `Refused set_policy: why` as `refused: why`.
fn refused(text: &str) -> String {
    let why = text.split_once(": ").map_or(text, |(_, why)| why);
    format!("refused: {why}")
}

/// The source of the content a row returns, if it returns content.
fn content_source(row: &agentabi::Action) -> Option<&'static str> {
    match (row.returns, row.ret) {
        (Returns::Content(source), _) => Some(source),
        (_, RetType::Data) => Some("data"),
        _ => None,
    }
}

/// A script value as JSON shaped by `ret`.
pub(crate) fn json_of(value: Option<&Dynamic>, ret: &RetType) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    match ret {
        RetType::Unit => Value::Null,
        RetType::Data => match value.clone().try_cast::<crate::data::Data>() {
            Some(data) => Value::String(data.text().to_string()),
            None => text(value),
        },
        RetType::Of(agentabi::ArgType::Int { .. }) => value
            .as_int()
            .map_or(Value::Null, |n| Value::Number(Number::from(n))),
        RetType::Of(_) => text(value),
        RetType::Record(fields) => {
            let Some(map) = value.clone().try_cast::<rhai::Map>() else {
                return Value::Null;
            };
            let mut out = Map::new();
            for (name, ty) in *fields {
                let field = map.get(*name);
                out.insert((*name).to_string(), json_of(field, ty));
            }
            Value::Object(out)
        }
        RetType::List(item) => match value.clone().try_cast::<rhai::Array>() {
            Some(items) => Value::Array(items.iter().map(|v| json_of(Some(v), item)).collect()),
            None => Value::Null,
        },
    }
}

fn text(value: &Dynamic) -> Value {
    match value.clone().into_string() {
        Ok(text) => Value::String(text),
        Err(_) => Value::String(value.to_string()),
    }
}
