//! The `model_*` rows (ADR 0008 §11; P4 task 10), which `updated` carries out
//! through the shell's route to it ([`crate::Host::models`]):
//!
//! - `model_list` answers what `updated` lists, `kai-models 1`, in words;
//! - `model_use` names a stored model the signed role takes for the next
//!   restart, on trial. It runs when the line names the model, and is logged
//!   with `Inverse::Model`, so `undo` puts the model before back;
//! - `model_fetch` imports the attached disk's model, held to the signed role.
//!   It always asks once: gigabytes of new bytes, and no undo yet.
//!
//! What `updated` answers is the OS's own state, so it is the reply as it is.

use crate::actions::{How, Pending};
use crate::actlog::{Change, Entry, Form, State};
use crate::script::Checked;
use crate::tiers::Tier;
use crate::undo::{Inverse, Refused};
use crate::{Reply, Shell};
use agentabi::consent::{Decision, Why};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use comp_models::{Models, Sha256, hex, size};
use modelchan::listing;

/// What waits on an asked model row's answer.
#[derive(Clone, Debug)]
pub(crate) enum Waiting {
    Use { to: Sha256, back: Option<Sha256> },
    Fetch,
}

impl Shell {
    fn models_now(&self) -> Result<Models, String> {
        let text = self.host.models("models")?;
        listing::parse(&text).map_err(|why| format!("updated's list: {why}"))
    }

    /// A `model_*` row the AI called, after consent.
    pub(crate) fn model_call(
        &mut self,
        id: u64,
        request: &str,
        row: &str,
        values: &[(&'static str, String)],
        decision: Decision,
    ) -> Reply {
        let name = values
            .iter()
            .find(|(n, _)| *n == "name")
            .map(|(_, v)| v.as_str())
            .unwrap_or_default();
        match row {
            "model_list" => match self.models_now() {
                Ok(models) => Reply::ok(listed(&models)),
                Err(why) => Reply::err(why),
            },
            "model_use" => self.model_use(id, request, name, decision),
            "model_fetch" => {
                let call = canonical("model_fetch", "disk");
                let why = match decision.why {
                    Why::TakenAs | Why::Grounded | Why::Read => Why::NoUndo,
                    why => why,
                };
                self.model_ask(id, request, &call, Waiting::Fetch, why)
            }
            other => Reply::err(format!("{other} is typed at the prompt for now")),
        }
    }

    fn model_use(&mut self, id: u64, request: &str, name: &str, decision: Decision) -> Reply {
        let models = match self.models_now() {
            Ok(models) => models,
            Err(why) => return Reply::err(why),
        };
        let Some(row) = models.find(name) else {
            return Reply::err(format!("no model {name}: model_list names them"));
        };
        let to = row.sha256;
        if models.use_model(&to).is_none() {
            let why = match models.next() == Some(to) {
                true => "the next restart uses it already",
                false => "it is not a stored model the signed role takes",
            };
            return Reply::err(format!("{}: {why}", row.name()));
        }
        let back = models.next();
        let call = canonical("model_use", name);
        match (decision.tier, decision.why) {
            // A model the AI picked for a vague line asks once.
            (Tier::Run, Why::TakenAs) => {
                self.model_ask(id, request, &call, Waiting::Use { to, back }, Why::TakenAs)
            }
            (Tier::Run, _) => {
                let how = How {
                    form: Form::Call,
                    tier: Tier::Run,
                    consent: String::from("asked"),
                };
                self.model_switch(id, request, &call, to, back, how)
            }
            (_, why) => self.model_ask(id, request, &call, Waiting::Use { to, back }, why),
        }
    }

    fn model_ask(
        &mut self,
        id: u64,
        request: &str,
        call: &str,
        waiting: Waiting,
        why: Why,
    ) -> Reply {
        self.host.audit(&format!("request {id}: {call} asks"));
        let reply = self.ask_call(id, request, Checked::shown_only(call), why, None, None);
        if let Some(pending) = &mut self.pending {
            pending.model = Some(waiting);
        }
        reply
    }

    /// The answer to a model row's menu: an Allow whose ONCE checks makes it.
    pub(crate) fn answer_model(&mut self, pending: Pending, text: &str) -> Reply {
        let id = pending.id;
        let Some(waiting) = pending.model.clone() else {
            return Reply::err("nothing to confirm");
        };
        if let Err(reply) = self.allowed(&pending, text) {
            return reply;
        }
        let how = How {
            consent: String::from("yes"),
            ..pending.how.clone()
        };
        let call = pending.checked.script.clone();
        let reply = match waiting {
            Waiting::Use { to, back } => {
                self.model_switch(id, &pending.request, &call, to, back, how)
            }
            Waiting::Fetch => self.model_fetch(id, &pending.request, &call, how),
        };
        self.remember(&pending, &reply.text);
        reply
    }

    /// `model_use` made, and logged with the model before as its undo.
    fn model_switch(
        &mut self,
        id: u64,
        request: &str,
        call: &str,
        to: Sha256,
        back: Option<Sha256>,
        how: How,
    ) -> Reply {
        let answer = match self.host.models(&format!("models use {}", hex(&to))) {
            Ok(answer) => answer,
            Err(why) => return Reply::err(why),
        };
        if !answer.starts_with("ok ") {
            return Reply::err(answer.trim_start_matches("err ").to_string());
        }
        let name = |sha: Option<Sha256>| sha.map_or_else(|| String::from("none"), |s| hex(&s));
        let change = Change {
            action: String::from("model_use"),
            owner: String::from("updated"),
            res: String::from("model"),
            before: name(back),
            after: name(Some(to)),
            inverse: Some(Inverse::Model { back, from: to }),
        };
        self.log_change(id, request, call, how, change);
        Reply::ok(String::from(
            "Done: the next restart uses it, on trial; the model before comes back if it does not load. undo",
        ))
    }

    fn model_fetch(&mut self, id: u64, request: &str, call: &str, how: How) -> Reply {
        let answer = match self.host.models("models import") {
            Ok(answer) => answer,
            Err(why) => return Reply::err(why),
        };
        let mut words = answer.split(' ');
        let (Some("ok"), Some("importing"), Some(target), Some(length)) =
            (words.next(), words.next(), words.next(), words.next())
        else {
            return Reply::err(answer.trim_start_matches("err ").to_string());
        };
        let length = length.parse().map_or_else(|_| String::from(length), size);
        let change = Change {
            action: String::from("model_fetch"),
            owner: String::from("updated"),
            res: String::from("model-store"),
            before: String::new(),
            after: String::from(target),
            inverse: None,
        };
        self.log_change(id, request, call, how, change);
        Reply::ok(format!(
            "Importing {target} ({length}) from the attached disk, checked against the signed role; model_list shows when it is stored"
        ))
    }

    fn log_change(&mut self, id: u64, request: &str, call: &str, how: How, change: Change) {
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: how.form,
            of: None,
            tier: Some(how.tier),
            pack: None,
            consent: how.consent,
            state: State::Done,
            request: String::from(request),
            script: String::from(call),
            outcome: String::from("ran"),
            changes: alloc::vec![change],
        });
    }

    /// `undo` of `model_use`: the model before for the next restart, while the
    /// next restart's model is still `from`, or (`anyway`) whatever it is.
    pub(crate) fn model_back(
        &mut self,
        back: Option<Sha256>,
        from: Sha256,
        anyway: bool,
    ) -> Result<String, Refused> {
        let back = back.ok_or_else(|| {
            Refused::Failed(String::from(
                "no model was in use before, so there is none to go back to",
            ))
        })?;
        let models = self.models_now().map_err(Refused::Failed)?;
        let next = models.next();
        if next != Some(from) && !anyway {
            let now = next.map_or_else(|| String::from("none"), |s| String::from(&hex(&s)[..12]));
            return Err(Refused::Moved(now));
        }
        let answer = self
            .host
            .models(&format!("models use {}", hex(&back)))
            .map_err(Refused::Failed)?;
        match answer.starts_with("ok ") {
            true => Ok(String::from("the model before, from the next restart")),
            false => Err(Refused::Failed(answer)),
        }
    }
}

/// The canonical call for a model row: `model_use("qwen3-1.7b")`.
fn canonical(action: &str, arg: &str) -> String {
    termcore::once::call_text(action, &[termcore::once::CallArg::Text(arg)])
        .unwrap_or_else(|| String::from(action))
}

/// The list in words, one model a line, as the model manager shows it.
fn listed(models: &Models) -> String {
    let rows = models.rows();
    if rows.is_empty() {
        return String::from("no models: none is stored and the signed role names none");
    }
    let lines: Vec<String> = rows
        .iter()
        .map(|r| {
            let state = match r.state {
                comp_models::State::Running if models.store.on_trial => "in use, on trial",
                comp_models::State::Running => "in use",
                comp_models::State::Next => "from the next restart",
                comp_models::State::Stored => "stored",
                comp_models::State::Available => "signed, not stored",
            };
            let id = r.signed.map_or(String::new(), |s| format!(" ({})", s.id));
            format!(
                "{}{id}: {state}, {}, {}",
                r.name(),
                size(r.length),
                r.licence()
            )
        })
        .collect();
    lines.join("\n")
}
