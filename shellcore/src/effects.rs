//! An app's effects (ADR 0003 §4; P4, for the Files, Monitor and Settings apps):
//! anything outside the app comes back in its reply as `effect <ticket> <wave>`
//! lines, `call({name: "ui_open", args: "{app: \"notes\"}"})` or
//! `notify("saved")`, and the agent core runs them after the call. Each comes
//! back to the app as `effect-done(ticket, result)`.
//!
//! The app's arguments are data (ADR 0002's provenance rule): app text alone
//! never makes an outbound, destructive or undo-less effect run. So an effect
//! runs only when its row runs at once on its own, local and undoable, and is
//! then logged and undoable like any call (`consent=app:<app>`). One that would
//! ask is refused: an app cannot ask the user on its own behalf in v0.1. Which
//! app it was comes from the caller's record, never from the lines.

use crate::actions::How;
use crate::actlog::Form;
use crate::script;
use crate::tiers::Tier;
use crate::{Reply, Shell};
use agentabi::{Action, ArgType, RetType};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use appabi::{Ty, Val, wave};
use llmproto::act::Call;
use llmproto::script::Answer;
use serde_json::{Map, Value};

/// Most effects one reply may carry (livecore's `MAX_EFFECTS`).
pub(crate) const MAX_EFFECTS: usize = 8;

/// What one effect came to, for the app's `effect-done`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectDone {
    pub ticket: u32,
    /// Code's result line, or why it did not run.
    pub outcome: Result<String, String>,
}

/// One effect as the app queued it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Effect {
    Call { name: String, args: String },
    Notify(String),
}

/// The WAVE type of an effect (ADR 0003's `variant effect`).
fn effect_ty() -> Ty {
    let call = Ty::Record(alloc::vec![
        (String::from("name"), Ty::String),
        (String::from("args"), Ty::String),
    ]);
    Ty::Variant(alloc::vec![
        (String::from("call"), Some(call)),
        (String::from("notify"), Some(Ty::String)),
    ])
}

/// `effect <ticket> <wave>`; `None` for any other line or one out of shape.
fn parse_line(line: &str) -> Option<(u32, Result<Effect, String>)> {
    let rest = line.strip_prefix("effect ")?;
    let (ticket, text) = rest.split_once(' ')?;
    let ticket = ticket.parse().ok()?;
    let effect = match wave::parse(text, &effect_ty()) {
        Ok(Val::Variant(0, Some(call))) => match *call {
            Val::Record(fields) => match fields.as_slice() {
                [Val::String(name), Val::String(args)] => Ok(Effect::Call {
                    name: name.clone(),
                    args: args.clone(),
                }),
                _ => Err(String::from("an effect call is {name, args}")),
            },
            _ => Err(String::from("an effect call is {name, args}")),
        },
        Ok(Val::Variant(1, Some(text))) => match *text {
            Val::String(text) => Ok(Effect::Notify(text)),
            _ => Err(String::from("a notify carries text")),
        },
        Ok(_) => Err(String::from("an effect is call or notify")),
        Err(why) => Err(format!("an effect is not WAVE: {why}")),
    };
    Some((ticket, effect))
}

/// The WAVE type a row's arguments take: a record of them, in the row's order.
fn args_ty(row: &Action) -> Ty {
    Ty::Record(
        row.args
            .iter()
            .map(|arg| (String::from(arg.name), ty_of(&arg.ty)))
            .collect(),
    )
}

fn ty_of(ty: &ArgType) -> Ty {
    match ty {
        ArgType::Int { .. } => Ty::S64,
        ArgType::Enum(_) | ArgType::Text { .. } | ArgType::Path | ArgType::Host => Ty::String,
        ArgType::List(inner, _) => Ty::List(Box::new(ty_of(inner))),
        ArgType::Opt(inner) => Ty::Option(Box::new(ty_of(inner))),
    }
}

/// A WAVE value as the JSON a tagged call carries; `None` for an absent option.
fn json_of(value: &Val) -> Option<Value> {
    match value {
        Val::String(s) => Some(Value::String(s.clone())),
        Val::S64(n) => Some(Value::from(*n)),
        Val::List(items) => Some(Value::Array(items.iter().filter_map(json_of).collect())),
        Val::Option(Some(inner)) => json_of(inner),
        _ => None,
    }
}

/// The effect as the tagged call the door takes, its arguments checked by the
/// same rules as a model's call.
fn call_of(row: &'static Action, args: &str) -> Result<Call, String> {
    let Val::Record(values) = wave::parse(args, &args_ty(row))
        .map_err(|why| format!("{}'s arguments: {why}", row.name))?
    else {
        return Err(format!("{}'s arguments are a record", row.name));
    };
    let mut arguments = Map::new();
    for (arg, value) in row.args.iter().zip(&values) {
        if let Some(json) = json_of(value) {
            arguments.insert(arg.name.to_string(), json);
        }
    }
    Ok(Call {
        name: row.name.to_string(),
        arguments,
    })
}

impl Shell {
    /// Runs what the apps' calls in the last run queued (P4): each through
    /// [`Shell::run_effects`], with its own tier, log and undo.
    pub(crate) fn run_app_effects(&mut self) {
        let Some(queued) = self
            .host
            .apps()
            .map(|apps| apps.borrow_mut().take_effects())
        else {
            return;
        };
        for (app, lines) in queued {
            for done in self.run_effects(&app, &lines) {
                if let Err(why) = done.outcome {
                    self.host.audit(&format!(
                        "{app}'s effect {} did not run: {why}",
                        done.ticket
                    ));
                }
            }
        }
    }

    /// Runs app `app`'s effect lines, oldest first: what each came to. `app` is
    /// the caller's record of who replied. Past [`MAX_EFFECTS`], the rest are
    /// refused, not run.
    pub fn run_effects(&mut self, app: &str, lines: &str) -> Vec<EffectDone> {
        let mut out = Vec::new();
        for (ticket, effect) in lines.lines().filter_map(parse_line) {
            let outcome = match (out.len() < MAX_EFFECTS, effect) {
                (false, _) => Err(format!("more than {MAX_EFFECTS} effects in one reply")),
                (true, Err(why)) => Err(why),
                (true, Ok(Effect::Notify(text))) => {
                    self.host.audit(&format!("{app} notes: {text}"));
                    Ok(String::new())
                }
                (true, Ok(Effect::Call { name, args })) => {
                    self.run_effect(app, ticket, &name, &args)
                }
            };
            if let Err(why) = &outcome {
                self.host
                    .audit(&format!("{app}'s effect {ticket} refused: {why}"));
            }
            out.push(EffectDone { ticket, outcome });
        }
        out
    }

    /// One `call` effect: at once if its row runs at once on its own, local and
    /// undoable; else refused.
    fn run_effect(
        &mut self,
        app: &str,
        ticket: u32,
        name: &str,
        args: &str,
    ) -> Result<String, String> {
        let row = agentabi::find(&agentabi::ROWS, name)
            .ok_or_else(|| format!("{name} is no action an app may call"))?;
        // Only a row the app's package declares for its effects, deny by
        // default: no app installs, removes or rules on its own (P4).
        crate::apps::load_app(self.host.as_ref(), app);
        let declared = self
            .host
            .apps()
            .is_some_and(|apps| apps.borrow().rows.may_effect(app, name));
        // A row the user withheld is refused as one never declared (P4 task 7).
        if !declared || crate::grants::is_withheld(self.host.as_ref(), app, name) {
            return Err(format!("{app} does not declare effects that call {name}"));
        }
        // A change derives Run only with an inverse, so this is local and undoable.
        let at_once = agentabi::derive(row) == Tier::Run;
        if !at_once || row.facts.open_world || row.facts.destructive {
            return Err(format!(
                "{name} asks the user first, which an app cannot do on its own; ask the AI"
            ));
        }
        // As a call's: a file row's script is checked among the file rows, any
        // other among the bound ones; packages go through the package door,
        // which an app's effect may not reach.
        let rows: &'static [Action] = match agentabi::FS_ROWS.iter().any(|r| r.name == row.name) {
            true => &agentabi::FS_ROWS,
            false => &agentabi::BOUND,
        };
        let is_pkg = agentabi::PKG_ROWS.iter().any(|r| r.name == row.name);
        if is_pkg || !rows.iter().any(|r| r.name == row.name) {
            return Err(format!("{name} is no action an app may call"));
        }
        let call = call_of(row, args)?;
        let args = crate::agentcore::arguments(row, &call)?;
        // What a read returns goes back to the app that asked, never to the
        // model: `show` puts it in the result.
        let called = format!("{}({})", row.name, args.rhai.join(", "));
        let script = match row.ret {
            RetType::Data => format!("show({called});"),
            _ => format!("{called};"),
        };
        let answer = Answer {
            script,
            summary: String::new(),
        };
        let checked = script::check_against(&answer, &self.host, &self.config, rows)
            .map_err(|refusal| refusal.to_string())?;
        let id = self.next_id;
        self.next_id += 1;
        let how = How {
            form: Form::Call,
            tier: Tier::Run,
            consent: format!("app:{app}"),
        };
        let request = format!("{app}: effect {ticket}");
        match self.finish(id, &request, &checked, how) {
            Reply { ok: true, text, .. } => Ok(text),
            Reply { text, .. } => Err(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_lines_read_as_calls_and_notes() {
        let call =
            parse_line(r#"effect 7 call({name: "set_policy", args: "{name: \"round-robin\"}"})"#);
        assert_eq!(
            call,
            Some((
                7,
                Ok(Effect::Call {
                    name: "set_policy".into(),
                    args: r#"{name: "round-robin"}"#.into(),
                })
            ))
        );
        assert_eq!(
            parse_line(r#"effect 8 notify("saved")"#),
            Some((8, Ok(Effect::Notify("saved".into()))))
        );
        assert!(matches!(parse_line("effect 9 shout(1)"), Some((9, Err(_)))));
        assert_eq!(parse_line("result ok"), None);
        assert_eq!(parse_line("effect x notify(\"a\")"), None);
    }
}
