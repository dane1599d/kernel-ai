//! The one door (ADR 0002 D2, D3 and D5): a request goes to the model as `act`
//! and comes back as one tagged call, and [`Shell::execute`] decides what it may
//! do. Code, never the model, decides:
//!
//! - **Consent** comes from the user's line (`agentabi::consent`): a row's cues
//!   with no negation in reach, `from_request` values literal in the line or
//!   aliased, the English check, and the vague path. Pasted text is part of the
//!   line: the terminal sends whatever was in it when Enter was pressed.
//! - **The tier** is the stricter of the row's derived tier and consent's.
//! - **The result line** is written from what ran: `Done: policy round-robin (was
//!   interactive). undo`, or `Taken as: …` for a vague line.
//!
//! A call to a script API row runs through the same strict engine, journal and
//! `undo` as a script, as a one-line script written by code. `say` answers in
//! words, `clarify` asks and joins the reply to the line, and `script` runs its
//! code through today's script checks. If no call comes back (no model, or a
//! server with no answer), today's script request runs instead.

use crate::actions::How;
use crate::actlog::Form;
use crate::script;
use crate::tiers::Tier;
use crate::{Reply, SCRIPT_ROUND_MS, Shell};
use agentabi::consent::{self, Call as Planned, Label, Value as Consented, Why};
use agentabi::{Action, RetType};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use llmproto::act::{ActError, ActRequest, Call};

/// The person's own folder: a file `ui_open` is given by name is in it.
const HOME: &str = "/home/user/";
use llmproto::script::Answer;
use serde_json::Value;

mod args;
mod ask;
mod memory;
mod target;

use args::text_arg;
use ask::{held, offer};

pub(crate) use args::{arguments, read_from};
pub(crate) use ask::{grant_reason, hold_reason, reason_of};
pub(crate) use memory::{Showed, remember};

/// What a call did, in code's terms; the reply is made from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The tier it ran, asked or was refused at.
    pub tier: Tier,
    /// Why, from consent and the row.
    pub why: Why,
    pub reply: Reply,
    /// A `c>` entry's call: its value as JSON, or why there is none; `None`
    /// while it asks (P4 task 12).
    pub value: Option<crate::centry::CallValue>,
}

/// How a call's arguments read as consent sees them, and as Rhai literals.
pub(crate) struct Arguments {
    values: Vec<(&'static str, String)>,
    pub(crate) rhai: Vec<String>,
}

impl Shell {
    /// Asks the model for one call for `request` and executes it; `None` when no
    /// call came back, so the caller falls back to today's script request.
    pub(crate) fn try_act(&mut self, id: u64, request: &str) -> Option<Reply> {
        let asked = ActRequest {
            line: request.to_string(),
            // Data, never consent (ADR 0010 §5): it reaches the model only.
            state: self.state_line().unwrap_or_default(),
            timeout_ms: SCRIPT_ROUND_MS,
            read: None,
            apps: crate::apps::wire(self.host.as_ref()),
        };
        // The apps' reads are answered while the model thinks.
        let host = Rc::clone(&self.host);
        let answer = host.act_serving(&asked, &mut |waiting| self.serve_waiting(waiting));
        // The person's Stop ended the call, or came while a model that had
        // already taken it answered: the line stops here, with no call run and
        // no script request after it (#887's reviews).
        let stopped = match &answer {
            Err(ActError::Stopped) => true,
            Ok(_) => self.host.interrupted(),
            Err(_) => false,
        };
        if stopped {
            self.host.audit(&format!("request {id}: stopped"));
            self.record(id, request, "", "stopped");
            return Some(Reply::err(String::from("stopped")));
        }
        let call = match answer {
            Ok(call) => call,
            // A model that answered but ran out of time, or cut its call off,
            // gets no second, slower try as a script request (S02-2): that
            // request pays its own prompt and the same time limit again.
            Err(error @ (ActError::Local(_) | ActError::CutOff)) => {
                self.host
                    .audit(&format!("request {id}: no call ({})", error.describe()));
                self.record(
                    id,
                    request,
                    "",
                    &format!("refused: no call ({})", error.describe()),
                );
                return Some(Reply::err(format!(
                    "the model gave no call: {}",
                    error.describe()
                )));
            }
            Err(error) => {
                self.host.audit(&format!(
                    "request {id}: no call ({}); asking for a script",
                    error.describe()
                ));
                return None;
            }
        };
        self.host
            .audit(&format!("request {id}: the model called {}", call.text()));
        // A view the model asked for comes back to it once (ADR 0005 §6).
        let call = match self.second_round(id, &asked, call) {
            crate::ui::Round::First(call) => call,
            crate::ui::Round::Second(call) => {
                self.steered = Some("ui");
                call
            }
            crate::ui::Round::Failed(reply) => {
                self.record(id, request, "", &format!("refused: {}", reply.text));
                return Some(reply);
            }
        };
        self.showed = Showed::default();
        let outcome = self.execute(id, request, &call);
        self.steered = None;
        let showed = core::mem::take(&mut self.showed);
        // A turn that asked is remembered once it is answered (`actions`).
        match &mut self.pending {
            Some(pending) if outcome.reply.confirm == Some(id) => {
                pending.call = Some(call.text());
            }
            _ => remember(
                self.host.as_ref(),
                request,
                &call.text(),
                &outcome.reply.text,
                &showed,
            ),
        }
        Some(outcome.reply)
    }

    /// The one door: what `call`, made for the user's `request`, may do, and doing
    /// it.
    pub(crate) fn execute(&mut self, id: u64, request: &str, call: &Call) -> Outcome {
        let forwarded = match self.open_with(id, call) {
            Ok(forwarded) => forwarded,
            Err(why) => return self.refuse(id, request, call, Why::NeverValue, &why),
        };
        let call = forwarded.as_ref().unwrap_or(call);
        match call.name.as_str() {
            "say" => self.pseudo_say(id, request, call),
            "clarify" => self.pseudo_clarify(id, request, call),
            "script" => self.pseudo_script(id, request, call),
            "undo" => self.pseudo_undo(call),
            name => match self.row(name) {
                Some(row) => self.call_row(id, request, row, call),
                None => self.refuse(id, request, call, Why::Unknown, "no such action"),
            },
        }
    }

    /// `ui_open(app, arg)` with an argument, as the app's own open row
    /// (founder test 2, item 6): "open ideas.txt in the editor" runs
    /// `editor_open_file("/home/user/ideas.txt")`, which consent then judges
    /// as any call. A name is taken under the person's home, as the editor's
    /// own path check wants; `..` is refused. `Ok(None)` when the call is not
    /// that, or the app declares no open row.
    fn open_with(&self, id: u64, call: &Call) -> Result<Option<Call>, String> {
        if call.name != agentabi::UI_OPEN.name {
            return Ok(None);
        }
        let (Some(arg), Some(app)) = (
            call.arguments.get("arg").and_then(Value::as_str),
            call.arguments.get("app").and_then(Value::as_str),
        ) else {
            return Ok(None);
        };
        let arg = arg.trim();
        if arg.is_empty() {
            return Ok(None);
        }
        if arg.split('/').any(|part| part == "..") {
            return Err(format!("{arg}: a path may not go up with .."));
        }
        let path = match arg.starts_with('/') {
            true => String::from(arg),
            false => format!("{HOME}{}", arg.trim_start_matches("./")),
        };
        let app =
            crate::ui::app_of(crate::ui::canonical_on(self.host.as_ref(), app)).replace('-', "_");
        let Some((name, row)) = [format!("{app}_open_file"), format!("{app}_open")]
            .into_iter()
            .find_map(|name| Some((name.clone(), self.row(&name)?)))
        else {
            return Ok(None);
        };
        let Some(first) = row.args.first() else {
            return Ok(None);
        };
        self.host.audit(&format!(
            "request {id}: ui_open({app}, {arg}) runs as {name}"
        ));
        let mut arguments = serde_json::Map::new();
        arguments.insert(String::from(first.name), Value::from(path));
        Ok(Some(Call { name, arguments }))
    }

    /// The row named `name`: the OS's, then an installed app's (P4).
    pub(crate) fn row(&self, name: &str) -> Option<&'static Action> {
        agentabi::find(&agentabi::ROWS, name).or_else(|| {
            crate::apps::load_for(self.host.as_ref(), name);
            let apps = self.host.apps()?;
            apps.borrow().rows.find(name).map(|r| r.row)
        })
    }

    /// The rows `row` is decided or checked among: an app's row alone, every
    /// other among the OS's `rows`.
    fn rows_for(&self, row: &'static Action, rows: &'static [Action]) -> &'static [Action] {
        match row.owner {
            agentabi::Owner::App(_) => core::slice::from_ref(row),
            _ => rows,
        }
    }

    fn pseudo_say(&mut self, id: u64, request: &str, call: &Call) -> Outcome {
        let text = text_arg(call, "text");
        let reply = self.say(id, request, text);
        outcome(Tier::Run, Why::Read, reply)
    }

    /// `clarify`: the question, with its options; the next line joins the request.
    fn pseudo_clarify(&mut self, id: u64, request: &str, call: &Call) -> Outcome {
        let question = text_arg(call, "question");
        let options: Vec<&str> = call
            .arguments
            .get("options")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        // An option no row backs ("increase CPU frequency") is not offered.
        let options: Vec<&str> = options
            .into_iter()
            .filter(|option| crate::abilities::backed(option))
            .collect();
        self.record(id, request, "", "asked the user to clarify");
        self.clarifying = Some(request.to_string());
        self.clarified_by_model = true;
        let text = match options.is_empty() {
            true => question,
            false => format!("{question} ({})", options.join(", ")),
        };
        outcome(Tier::Run, Why::Read, Reply::ok(text))
    }

    /// A call whose named argument is a placeholder (`pkg_remove("package_name")`)
    /// does not run: the shell asks which one, and the answer joins the request
    /// as a `clarify` answer does. Only a target is checked, and an enum one
    /// is checked by its type.
    fn placeholder_question(
        &mut self,
        id: u64,
        request: &str,
        row: &'static Action,
        args: &Arguments,
    ) -> Option<Reply> {
        let values = args
            .values
            .iter()
            .map(|(name, value)| (*name, value.as_str()));
        let (arg, value) = crate::guard::placeholder_target(row, values, request)?;
        self.host.audit(&format!(
            "request {id}: {}'s {arg} is a placeholder; asking which",
            row.name
        ));
        self.record(id, request, "", "asked which: a placeholder argument");
        self.clarifying = Some(request.to_string());
        Some(Reply::ok(crate::guard::which(arg, value)))
    }

    /// `undo`: what the typed `undo` does, compare-and-set and all, so it only
    /// puts back what the log holds. One step at a time: a second `undo` redoes,
    /// so more steps are refused with how to name an entry instead.
    fn pseudo_undo(&mut self, call: &Call) -> Outcome {
        let steps = call.arguments.get("steps").and_then(Value::as_u64);
        let reply = match steps {
            // Never one that gives an app back a network grant: only the
            // person's typed `undo` or Settings may (K on #750).
            None | Some(1) => match self.kai_undo_refusal() {
                Some(why) => Reply::err(why),
                None => self.undo_command(""),
            },
            Some(_) => Reply::err(
                "I undo one change at a time: `log` lists them, and `undo N` puts back entry N",
            ),
        };
        outcome(Tier::Run, Why::Read, reply)
    }

    /// `script`: code written by today's script prompt, checked and tiered as
    /// today's scripts are. No model summary: the card shows the code.
    fn pseudo_script(&mut self, id: u64, request: &str, call: &Call) -> Outcome {
        let answer = Answer {
            script: text_arg(call, "code"),
            summary: String::new(),
        };
        let checked = match script::check(&answer, &self.host, &self.config) {
            Ok(checked) => checked,
            Err(refusal) => {
                let why = refusal.to_string();
                self.record(id, request, &answer.script, &format!("refused: {why}"));
                return outcome(Tier::Never, Why::Unknown, Reply::err(why));
            }
        };
        if let Some(why) = crate::guard::in_plan(&checked.plan, request) {
            self.record(id, request, &checked.script, &format!("refused: {why}"));
            return outcome(Tier::Never, Why::Unknown, Reply::err(why));
        }
        if let Some(why) = self.told_only(request, &checked) {
            self.record(id, request, &checked.script, &format!("refused: {why}"));
            return outcome(Tier::Never, Why::Added, Reply::err(why));
        }
        let (tier, at) = self.tier_of(request, &checked);
        if tier == Tier::Never {
            let why = format!("{} may not be used in scripts here", at.join(", "));
            self.record(id, request, &checked.script, &format!("refused: {why}"));
            return outcome(Tier::Never, Why::NeverValue, Reply::err(why));
        }
        let reply = self.propose(id, request, checked, (tier, at));
        outcome(tier, Why::Asked, reply)
    }

    /// A registry row: consent decides, then the row runs as a one-line script.
    fn call_row(&mut self, id: u64, request: &str, row: &'static Action, call: &Call) -> Outcome {
        // A restart is the person's own line (`power`), never a model's call.
        if row.name == agentabi::POWER_REBOOT.name {
            let why = "a restart is typed by the person: restart";
            return self.refuse(id, request, call, Why::Unknown, why);
        }
        let (row, subject) = match self.target(id, request, row, call) {
            Ok(found) => found,
            Err(outcome) => return outcome,
        };
        // An agent's step: the row that runs, which `target` or `ui_open`'s
        // forwarding may have changed, is admitted against its grants too.
        if let Err(why) = self.readmit(row) {
            return self.refuse(id, request, call, Why::Unknown, &why);
        }
        let args = match arguments(row, call) {
            Ok(args) => args,
            Err(why) => return self.refuse(id, request, call, Why::Unknown, &why),
        };
        if let Some(reply) = self.placeholder_question(id, request, row, &args) {
            return outcome(Tier::Run, Why::Read, reply);
        }
        let values: Vec<(&str, Consented)> = args
            .values
            .iter()
            .map(|(name, text)| {
                let value = Consented {
                    text,
                    label: Label::Literal,
                };
                (*name, value)
            })
            .collect();
        // A call made after reading a view was decided by data (ADR 0002 D6),
        // as are a `c>` entry's later calls after it read content.
        let seen = self.c_entry.as_ref().and_then(|entry| entry.seen);
        let control = self.steered.or(seen).map_or(Label::Literal, Label::Content);
        let planned = [Planned {
            name: row.name,
            args: &values,
            control,
        }];
        let granted = crate::appgrants::AppGrants::new(self.host.as_ref(), &self.grant_memo);
        let net = crate::analyse::grounding(&self.hosts, &granted);
        let decision = match row.owner {
            agentabi::Owner::App(_) => {
                let rows: Vec<Action> = agentabi::ROWS.iter().copied().chain([*row]).collect();
                consent::decide_net(request, &planned, &rows, &net)[0]
            }
            _ => consent::decide_net(request, &planned, &agentabi::ROWS, &net)[0],
        };
        if decision.tier == Tier::Never {
            let detail = match decision.why {
                Why::SendHost => "a send goes only to a site the request names",
                Why::SendUnasked => "the request did not ask to send anything",
                _ => "it is never run",
            };
            return self.refuse(id, request, call, decision.why, detail);
        }
        // A change for a line that only asks to be told answers nothing it
        // asked, whatever the State or a page told the model (#721): no ask.
        if decision.why == Why::Added && !row.facts.read_only && consent::asks_to_be_told(request) {
            let why = crate::actions::told_why(row.name);
            self.record(id, request, &call.text(), &format!("refused: {why}"));
            return outcome(Tier::Never, Why::Added, Reply::err(why));
        }
        if agentabi::GOAL_ROWS.iter().any(|r| r.name == row.name) {
            let reply = self.goal_call(id, request, row.name, &args.values, decision);
            return outcome(decision.tier, decision.why, reply);
        }
        if agentabi::PKG_ROWS.iter().any(|r| r.name == row.name) {
            let reply = self.pkg_call(id, request, row.name, &args.values, decision);
            return outcome(decision.tier, decision.why, reply);
        }
        if agentabi::MODEL_ROWS.iter().any(|r| r.name == row.name) {
            let reply = self.model_call(id, request, row.name, &args.values, decision);
            return outcome(decision.tier, decision.why, reply);
        }
        if row.name == agentabi::NET_SEND.name {
            // A send is asked for at Kai's prompt: a `c>` program may not make
            // one (ADR 0012), whatever its entry read.
            if self.c_entry.is_some() {
                let why = "a send is asked for at Kai's prompt, not from a c> program";
                return self.refuse(id, request, call, decision.why, why);
            }
            let reply = self.send_ask(id, request, &args.values, decision.why);
            return outcome(decision.tier, decision.why, reply);
        }
        if row.name == agentabi::APP_INSTALL.name {
            let reply = self.app_install_call(id, request, &args.values, decision);
            return outcome(decision.tier, decision.why, reply);
        }
        if agentabi::APP_ROWS.iter().any(|r| r.name == row.name) {
            let reply = self.app_call(id, request, &args.values, decision);
            return outcome(decision.tier, decision.why, reply);
        }
        // A file or window row's one-line script is found among its own rows,
        // every other row's among the bound ones.
        let own = |rows: &[Action]| rows.iter().any(|r| r.name == row.name);
        let rows: &'static [Action] = if own(&agentabi::FS_ROWS) {
            &agentabi::FS_ROWS
        } else if own(&agentabi::UI_ROWS) {
            &agentabi::UI_ROWS
        } else if own(&agentabi::GRANT_ROWS) {
            &agentabi::GRANT_ROWS
        } else if own(&agentabi::NET_ROWS) {
            &agentabi::NET_ROWS
        } else {
            self.rows_for(row, &agentabi::BOUND)
        };
        if !rows.iter().any(|r| r.name == row.name) {
            let why = format!("{} is typed at the prompt for now, not called", row.name);
            return self.refuse(id, request, call, decision.why, &why);
        }
        // What a content read returns is `Data`: `show` is the one way it comes
        // out, to the user and not back to the model (ADR 0002 D6).
        let called = format!("{}({})", row.name, args.rhai.join(", "));
        // A `c>` entry's call gives its value back to the program; anyone
        // else's Data is shown to the user only.
        let script = match (row.ret, self.c_entry.is_some()) {
            (_, true) => called,
            (RetType::Data, false) => format!("show({called});"),
            (_, false) => format!("{called};"),
        };
        let answer = Answer {
            script,
            summary: String::new(),
        };
        let mut checked = match script::check_against(&answer, &self.host, &self.config, rows) {
            Ok(checked) => checked,
            Err(refusal) => {
                return self.refuse(id, request, call, decision.why, &refusal.to_string());
            }
        };
        checked.person = self.person_line;
        self.host.audit(&format!(
            "request {id}: {} is {} ({:?})",
            row.name,
            decision.tier.name(),
            decision.why
        ));
        let offer = offer(row, &args, decision.why);
        // The person's Always rule for a host waives a grant read's ask as it
        // waived the ask for that host before grants grounded anything; never
        // a line that said to stay offline (ADR 0002 D3).
        let waiver = match decision.why {
            Why::GrantPath | Why::GrantSteered(_) | Why::GrantMany => {
                self::offer(row, &args, Why::Ungrounded)
            }
            _ => None,
        };
        // A send always asks (ADR 0002 question 3): no rule runs a Hold row,
        // whatever offer a rule was written from.
        let ruled = offer
            .as_ref()
            .or(waiver.as_ref())
            .filter(|_| !held(row))
            // The person's rules are for Kai's lines: an agent's step asks.
            .filter(|_| self.agent_acting().is_none())
            .and_then(|offer| self.rules.covering(offer));
        let reply = match (decision.tier, ruled) {
            (Tier::Run, _) => {
                self.shown = Some(script::shown(id, &checked));
                let consent = match decision.why {
                    Why::TakenAs => "taken-as",
                    _ => "asked",
                };
                let how = How {
                    form: Form::Call,
                    tier: Tier::Run,
                    consent: String::from(consent),
                };
                self.finish(id, request, &checked, how)
            }
            // A standing rule waives the ask the row itself makes (ADR 0002 D2a).
            (_, Some(rule)) => {
                self.shown = Some(format!(
                    "{}\nran under rule #{rule}",
                    script::shown(id, &checked)
                ));
                self.host.audit(&format!(
                    "request {id}: {} runs under rule #{rule}",
                    row.name
                ));
                let how = How {
                    form: Form::Call,
                    tier: decision.tier,
                    consent: format!("rule:{rule}"),
                };
                self.finish(id, request, &checked, how)
            }
            _ => {
                let reason = hold_reason(row);
                self.ask_call_with(id, request, checked, decision.why, offer, subject, reason)
            }
        };
        outcome(decision.tier, decision.why, reply)
    }

    fn refuse(&mut self, id: u64, request: &str, call: &Call, why: Why, detail: &str) -> Outcome {
        self.record(id, request, &call.text(), &format!("refused: {detail}"));
        let reply = Reply::err(format!("Refused {}: {detail}", call.name));
        outcome(Tier::Never, why, reply)
    }
}

#[cfg(test)]
#[path = "tests/agentcore_hold.rs"]
mod hold_tests;

fn outcome(tier: Tier, why: Why, reply: Reply) -> Outcome {
    Outcome {
        tier,
        why,
        reply,
        value: None,
    }
}

/// A call refused before any row was found.
pub(crate) fn outcome_refused(why: &str) -> Outcome {
    let mut refused = outcome(Tier::Never, Why::Unknown, Reply::err(why));
    refused.value = Some(Err(alloc::format!("refused: {why}")));
    refused
}
