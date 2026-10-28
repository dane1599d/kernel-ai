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
use crate::rules::Offer;
use crate::script::{self, Checked};
use crate::targets::{self, Unresolved, Verb};
use crate::tiers::Tier;
use crate::{Reply, SCRIPT_ROUND_MS, Shell};
use agentabi::consent::{self, Call as Planned, Label, Value as Consented, Why};
use agentabi::{Action, ArgType, RetType, Role};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use llmproto::act::{ActError, ActRequest, Call};
use llmproto::script::Answer;
use serde_json::Value;
use termcore::menu::{Menu, Origin};

/// What a call did, in code's terms; the reply is made from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The tier it ran, asked or was refused at.
    pub tier: Tier,
    /// Why, from consent and the row.
    pub why: Why,
    pub reply: Reply,
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
        };
        let call = match self.host.act(&asked) {
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
        let outcome = self.execute(id, request, &call);
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
            ),
        }
        Some(outcome.reply)
    }

    /// The one door: what `call`, made for the user's `request`, may do, and doing
    /// it.
    pub(crate) fn execute(&mut self, id: u64, request: &str, call: &Call) -> Outcome {
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
        self.record(id, request, "", "asked the user to clarify");
        self.clarifying = Some(request.to_string());
        let text = match options.is_empty() {
            true => question,
            false => format!("{question} ({})", options.join(", ")),
        };
        outcome(Tier::Run, Why::Read, Reply::ok(text))
    }

    /// `undo`: what the typed `undo` does, compare-and-set and all, so it only
    /// puts back what the log holds. One step at a time: a second `undo` redoes,
    /// so more steps are refused with how to name an entry instead.
    fn pseudo_undo(&mut self, call: &Call) -> Outcome {
        let steps = call.arguments.get("steps").and_then(Value::as_u64);
        let reply = match steps {
            None | Some(1) => self.undo_command(""),
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
        let (row, subject) = match self.target(id, request, row, call) {
            Ok(found) => found,
            Err(outcome) => return outcome,
        };
        let args = match arguments(row, call) {
            Ok(args) => args,
            Err(why) => return self.refuse(id, request, call, Why::Unknown, &why),
        };
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
        let planned = [Planned {
            name: row.name,
            args: &values,
            control: Label::Literal,
        }];
        let decision = consent::decide(request, &planned, self.rows_for(row, &agentabi::ROWS))[0];
        if decision.tier == Tier::Never {
            return self.refuse(id, request, call, decision.why, "it is never run");
        }
        if agentabi::PKG_ROWS.iter().any(|r| r.name == row.name) {
            let reply = self.pkg_call(id, request, row.name, &args.values, decision);
            return outcome(decision.tier, decision.why, reply);
        }
        // A file row's one-line script is found among the file rows, every other
        // row's among the bound ones.
        let rows: &'static [Action] = match agentabi::FS_ROWS.iter().any(|r| r.name == row.name) {
            true => &agentabi::FS_ROWS,
            false => self.rows_for(row, &agentabi::BOUND),
        };
        if !rows.iter().any(|r| r.name == row.name) {
            let why = format!("{} is typed at the prompt for now, not called", row.name);
            return self.refuse(id, request, call, decision.why, &why);
        }
        // What a content read returns is `Data`: `show` is the one way it comes
        // out, to the user and not back to the model (ADR 0002 D6).
        let called = format!("{}({})", row.name, args.rhai.join(", "));
        let script = match row.ret {
            RetType::Data => format!("show({called});"),
            _ => format!("{called};"),
        };
        let answer = Answer {
            script,
            summary: String::new(),
        };
        let checked = match script::check_against(&answer, &self.host, &self.config, rows) {
            Ok(checked) => checked,
            Err(refusal) => {
                return self.refuse(id, request, call, decision.why, &refusal.to_string());
            }
        };
        self.host.audit(&format!(
            "request {id}: {} is {} ({:?})",
            row.name,
            decision.tier.name(),
            decision.why
        ));
        let offer = offer(row, &args, decision.why);
        let ruled = offer.as_ref().and_then(|offer| self.rules.covering(offer));
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
            _ => self.ask_call(id, request, checked, decision.why, offer, subject),
        };
        outcome(decision.tier, decision.why, reply)
    }

    /// An ask-tier call waits for an answer from the Allow / Deny menu, or for `y`
    /// or `n` when its question cannot be shown as a menu.
    pub(crate) fn ask_call(
        &mut self,
        id: u64,
        request: &str,
        checked: Checked,
        why: Why,
        offer: Option<Offer>,
        subject: Option<String>,
    ) -> Reply {
        let reason = match why {
            Why::NotEnglish => String::from("the request is not in English, so it asks first"),
            Why::Added => String::from("the request did not ask for it"),
            Why::NoUndo => String::from("it has no undo"),
            Why::Steered(source) | Why::Outbound(source) => format!("{source} chose it"),
            Why::Ungrounded => String::from("the request did not name that site"),
            _ => String::from("it asks first"),
        };
        let call = checked.script.trim_end_matches(';').to_string();
        let changes: Vec<&'static str> = checked.changes.clone();
        self.shown = Some(format!(
            "{}\nasked first: {reason}",
            script::shown(id, &checked)
        ));
        self.pending = Some(crate::actions::Pending {
            id,
            request: request.to_string(),
            checked,
            asking: changes,
            created_ns: self.host.now_ns(),
            how: How {
                form: Form::Call,
                tier: Tier::Ask,
                consent: String::from("asked"),
            },
            call: None,
            offer: offer.clone(),
            menu: None,
            pkg: None,
            standing: None,
        });
        // The highlight starts on Allow only for a call the line plainly asked for.
        let origin = match why {
            Why::NoUndo | Why::Ungrounded => Origin::Asked,
            _ => Origin::Added,
        };
        let (kind, target) = offer.map(|o| o.labels()).unwrap_or_default();
        let question = match subject {
            // The call stays word for word: the ONCE an Allow mints names it.
            Some(subject) => format!("{subject} with {call}? {}.", capitalised(&reason)),
            None => format!("Run {call}? {}.", capitalised(&reason)),
        };
        let menu = Menu::new(id, &question, Some(&kind), Some(&target), origin)
            .and_then(|menu| menu.with_call(&call));
        match menu {
            Ok(_) => {
                let text = Menu::encode(id, origin, &question, &kind, &target, &call);
                if let Some(pending) = &mut self.pending {
                    pending.menu = Some(call);
                }
                Reply::menu(id, text)
            }
            // Too long, or not plain ASCII, for the menu: the y/n question shows it.
            Err(_) => Reply {
                ok: true,
                text: format!("Run {call}? {reason}. y/n, or show"),
                confirm: Some(id),
                more: false,
            },
        }
    }

    /// The row a stop, start or restart runs: code looks the name up among the
    /// supervisor's services and the running processes, whichever row the model
    /// picked, and asks when the name is both or neither. Any other row, a
    /// protected name, or a host that cannot list services keeps the model's row.
    /// With the row comes how the menu names the target ("Stop service indexer").
    fn target(
        &mut self,
        id: u64,
        request: &str,
        row: &'static Action,
        call: &Call,
    ) -> Result<(&'static Action, Option<String>), Outcome> {
        let kept = Ok((row, None));
        let Some(verb) = Verb::of(row) else {
            return kept;
        };
        let name = text_arg(call, "name");
        if agentabi::PROTECTED.contains(&name.as_str()) {
            return kept;
        }
        let (Ok(services), Ok(metrics)) = (self.host.services(), self.host.metrics()) else {
            return kept;
        };
        let processes: Vec<(u64, String)> = metrics
            .processes
            .iter()
            .take(metrics.process_count as usize)
            .map(|p| (p.pid, p.name().to_string()))
            .collect();
        match targets::resolve(verb, &name, &services, &processes) {
            Ok(found) => {
                if found.row.name != row.name {
                    self.host.audit(&format!(
                        "request {id}: {} on {name} runs as {}",
                        row.name, found.row.name
                    ));
                }
                Ok((found.row, Some(found.question())))
            }
            Err(Unresolved::Clarify(question)) => {
                self.record(id, request, &call.text(), "asked the user to clarify");
                self.clarifying = Some(request.to_string());
                Err(outcome(Tier::Run, Why::Read, Reply::ok(question)))
            }
            Err(Unresolved::Say(text)) => {
                let reply = self.say(id, request, text);
                Err(outcome(Tier::Run, Why::Read, reply))
            }
        }
    }

    fn refuse(&mut self, id: u64, request: &str, call: &Call, why: Why, detail: &str) -> Outcome {
        self.record(id, request, &call.text(), &format!("refused: {detail}"));
        let reply = Reply::err(format!("Refused {}: {detail}", call.name));
        outcome(Tier::Never, why, reply)
    }
}

/// The standing rules a menu may offer for a call that asks for `why`: the row's
/// own ask (no undo) takes a rule for the action or for its target; a host nobody
/// named takes one for that host only. Asks from consent take none.
fn offer(row: &'static Action, args: &Arguments, why: Why) -> Option<Offer> {
    let target = row
        .args
        .iter()
        .find(|arg| arg.role == Role::Target)
        .and_then(|arg| args.values.iter().find(|(name, _)| *name == arg.name))
        .map(|(_, text)| text.clone());
    match why {
        Why::NoUndo => Some(Offer {
            action: row.name,
            target,
            kind: true,
        }),
        Why::Ungrounded => target.map(|target| Offer {
            action: row.name,
            target: Some(target),
            kind: false,
        }),
        _ => None,
    }
}

/// `text` with its first letter in capitals, to start the menu's sentence.
fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Longest result line llm keeps for a turn.
const RESULT_MAX_BYTES: usize = 300;

/// Sends a finished model turn to llm's conversation memory: the user's line, the
/// call as `act` returned it, and the result line, its last line of at most 300
/// bytes (`Done: …`, `Taken as: …`, `Refused …`, `Not run: …`).
pub(crate) fn remember(host: &dyn crate::Host, line: &str, call: &str, reply: &str) {
    let last = reply
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    let result = &last[..last.floor_char_boundary(RESULT_MAX_BYTES)];
    host.remember(line, call, result);
}

fn outcome(tier: Tier, why: Why, reply: Reply) -> Outcome {
    Outcome { tier, why, reply }
}

/// A string argument, or "" when the model left it out.
fn text_arg(call: &Call, name: &str) -> String {
    call.arguments
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A call's arguments in the row's order, checked against its types: as text for
/// consent and as Rhai literals for the one-line script.
pub(crate) fn arguments(row: &'static Action, call: &Call) -> Result<Arguments, String> {
    if let Some(extra) = call
        .arguments
        .keys()
        .find(|key| !row.args.iter().any(|arg| arg.name == key.as_str()))
    {
        return Err(format!("{} has no argument {extra:?}", row.name));
    }
    let mut args = Arguments {
        values: Vec::new(),
        rhai: Vec::new(),
    };
    for arg in row.args {
        let Some(value) = call.arguments.get(arg.name) else {
            if matches!(arg.ty, ArgType::Opt(_)) {
                continue;
            }
            return Err(format!("{} needs its {} argument", row.name, arg.name));
        };
        let (text, rhai) = literal(&arg.ty, value)
            .ok_or_else(|| format!("{} is not a valid {}", value, arg.name))?;
        args.values.push((arg.name, text));
        args.rhai.push(rhai);
    }
    Ok(args)
}

/// `value` as text and as a Rhai literal, if it has the type `ty` allows.
fn literal(ty: &ArgType, value: &Value) -> Option<(String, String)> {
    match (ty, value) {
        (ArgType::Opt(inner), value) => literal(inner, value),
        (ArgType::Int { min, max }, Value::Number(n)) => {
            let n = n.as_i64().filter(|n| (*min..=*max).contains(n))?;
            Some((n.to_string(), n.to_string()))
        }
        (ArgType::Enum(words), Value::String(s)) if words.contains(&s.as_str()) => {
            Some((s.clone(), quote(s)?))
        }
        (ArgType::Text { max }, Value::String(s)) if s.chars().count() <= usize::from(*max) => {
            Some((s.clone(), quote(s)?))
        }
        (ArgType::Path | ArgType::Host, Value::String(s)) => Some((s.clone(), quote(s)?)),
        _ => None,
    }
}

/// A Rhai string literal for plain one-line text, quoted as the canonical call
/// text the ONCE digest names (`termcore::once::quote`).
fn quote(s: &str) -> Option<String> {
    termcore::once::quote(s)
}
