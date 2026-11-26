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
use crate::actlog::{Entry, Form, State, clip};
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
    request: String,
    /// The source of the first content a call returned, if any did.
    pub(crate) seen: Option<&'static str>,
    /// Kai's program, when the entry is one (`cmode::program`): the log
    /// records it in place of the calls' one-line scripts.
    pub(crate) program: Option<String>,
    /// The ids of the entry's own calls: only their log entries join it,
    /// never a Settings click, an app's effect or an agent's step logged
    /// while it ran or waited (bug-hunt-3 H3-21).
    ids: Vec<u64>,
    /// Its calls' log entries so far, oldest first, taken out of the log as
    /// each call ends, so an entry of more calls than the log keeps never
    /// pushes its own first changes, or older entries, out of it (H3-23).
    parts: Vec<Entry>,
    /// The id of the stand-in [`Shell::entry_stand_in`] left in the log while
    /// the entry waits on an answer, if one is there.
    stand_in: Option<u64>,
}

/// What a call of an entry came to, for `ccserve`'s `resume`.
pub type CallValue = Result<Value, String>;

/// Whether `name` is a row a `c>` entry may call: the bound OS rows and the
/// file and package rows, as `kai.h` declares them.
pub(crate) fn kai_row(name: &str) -> bool {
    [
        &agentabi::BOUND[..],
        &agentabi::FS_ROWS,
        &agentabi::PKG_ROWS,
    ]
    .iter()
    .any(|rows| rows.iter().any(|row| row.name == name))
}

impl Shell {
    /// Begins C entry `request`, as the user typed it: its token.
    pub fn entry_begin(&mut self, request: &str) -> u64 {
        let start = self.log.last_id().max(self.next_id.saturating_sub(1));
        self.c_entry = Some(CEntry {
            request: request.to_string(),
            seen: None,
            program: None,
            ids: Vec::new(),
            parts: Vec::new(),
            stand_in: None,
        });
        start
    }

    /// One `kai` call of the running entry: what it came to, with its value
    /// unless it asks (then [`Shell::take_value`] has it once answered).
    pub fn entry_call(&mut self, call: &Call) -> Outcome {
        let Some(request) = self.c_entry.as_ref().map(|e| e.request.clone()) else {
            return crate::agentcore::outcome_refused("no C entry is running");
        };
        // A second guard behind the C checker's: only the rows `kai.h` binds.
        if !kai_row(&call.name) {
            let why = format!("{} is not a kai call", call.name);
            return crate::agentcore::outcome_refused(&why);
        }
        let id = self.next_id;
        self.next_id += 1;
        if let Some(entry) = self.c_entry.as_mut() {
            entry.ids.push(id);
        }
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
        // Its log entries join the entry now; one that waits on an answer
        // leaves the calls so far in the log as one stand-in.
        self.entry_gather();
        if outcome.value.is_none() {
            self.entry_stand_in();
        }
        outcome
    }

    /// Ends the running entry: its calls' log entries become one, which an
    /// entry that failed (`ok` false) puts back at once. Its id, if it changed
    /// anything; an entry that only read is logged too.
    pub fn entry_end(&mut self, ok: bool) -> Option<u64> {
        self.entry_gather();
        let entry = self.c_entry.take()?;
        if entry.parts.is_empty() {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        let joined = merged(&entry, id, self.host.now_ns());
        let state = joined.state;
        self.log.push(joined);
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

    /// Takes the running entry's own calls' log entries out of the log into
    /// it. The stand-in [`Shell::entry_stand_in`] left there comes back as
    /// the calls so far, with whatever was done to it meanwhile: a file
    /// change that did not land is gone from it (`ledger::take_not_landed`),
    /// so it never comes back from the parts it was made of.
    fn entry_gather(&mut self) {
        let Some(entry) = self.c_entry.as_mut() else {
            return;
        };
        for mut taken in self.log.take_entries_of(&entry.ids) {
            // `merged` takes the request from the entry: a part never needs
            // its own copy, which up to MAX_CALLS parts would hold.
            taken.request = String::new();
            match entry.stand_in {
                Some(id) if id == taken.id => {
                    entry.stand_in = None;
                    entry.parts = alloc::vec![taken];
                }
                _ => entry.parts.push(taken),
            }
        }
    }

    /// While the entry waits on an answer, the log holds its calls so far as
    /// one entry under its last call's id, so a shell that stops meanwhile
    /// keeps their undo. The next gather takes it back in their place.
    fn entry_stand_in(&mut self) {
        let Some(entry) = self.c_entry.as_mut() else {
            return;
        };
        let Some(last) = entry.parts.last() else {
            return;
        };
        let stand_in = merged(entry, last.id, last.t_ns);
        entry.stand_in = Some(stand_in.id);
        self.log.push(stand_in);
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

/// `entry`'s calls as one log entry `id`: their changes, scripts and
/// outcomes, the highest tier, and the request as the user typed it.
fn merged(entry: &CEntry, id: u64, t_ns: u64) -> Entry {
    let parts = &entry.parts;
    let changes: Vec<_> = parts.iter().flat_map(|p| p.changes.clone()).collect();
    let scripts: Vec<&str> = parts.iter().map(|p| p.script.as_str()).collect();
    let outcome: Vec<&str> = parts.iter().map(|p| p.outcome.as_str()).collect();
    let state = match changes.is_empty() {
        true => State::Nothing,
        false => State::Done,
    };
    let script = match &entry.program {
        Some(program) => clip(program),
        None => clip(&scripts.join("\n")),
    };
    Entry {
        agent: agentid::Name::kai(),
        id,
        t_ns,
        form: Form::Call,
        of: None,
        tier: parts.iter().filter_map(|p| p.tier).max(),
        pack: None,
        consent: String::from(match entry.program {
            Some(_) => "c-program",
            None => "c-entry",
        }),
        state,
        request: entry.request.clone(),
        script,
        outcome: clip(&outcome.join("; ")),
        changes,
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
