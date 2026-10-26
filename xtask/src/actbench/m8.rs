//! The M8 fluency tasks inside actbench (ADR 0010 S10-1): the gate's own
//! harness ([`crate::fluency::attempt`]: the shell, its checks, one repair and
//! the judge) with the model's answers coming from an arm. Arm `m8` is today's
//! prompt and envelope byte for byte, `m8-a0` the same without `/no_think`; a
//! tagged arm answers with its call, which becomes the script the shell gets: a
//! `script` call's code as it is, `say` and `clarify` as an answer in words, and
//! a row's call as a one-line script that makes it.

use super::{Decoded, Tagged, base_turns, decode, user_text};
use crate::fluency::host::{GateHost, Transport};
use crate::fluency::local::Shared;
use infer::Next;
use llmproto::act::{self, ActError, ActRequest, History};
use llmproto::local;
use llmproto::script::{Answer, AnswerError, ScriptRequest};
use serde_json::{Value, json};
use std::rc::Rc;

/// What a tagged arm needs to answer a script request.
pub struct Tagging {
    pub engine: Shared,
    pub grammar: Rc<Tagged>,
    pub pack: Rc<String>,
    pub seed: u64,
    pub no_think: bool,
    /// A `script` call's code comes from today's prompt (arm `+m8script`).
    pub m8script: bool,
}

/// Today's path: `llmproto::local::script`, with or without `/no_think`.
pub fn reference(engine: Shared, no_think: bool) -> Transport {
    Box::new(move |request: &ScriptRequest| {
        let request = ScriptRequest::decode(&request.encode())
            .ok_or_else(|| AnswerError::Server("malformed script request".into()))?;
        local::script_with(&mut engine.borrow_mut(), &request, no_think, |_| {
            Next::Continue
        })
    })
}

/// The OS's own path: the shell's one door asks `act` first, answered as llm
/// answers it (`llmproto::local::act::routed` on the OS pack, through the wire
/// both ways), and scripts (the fallback and repair rounds) as llm
/// answers them today.
pub fn shipping(engine: Shared, grammar: Rc<infer::tagged::Tagged<'static>>) -> GateHost {
    let act_engine = engine.clone();
    let act = Box::new(move |request: &ActRequest| {
        let request = ActRequest::decode(&request.encode()).ok_or(ActError::NotACall)?;
        // Each M8 task is one turn on a fresh host: no history.
        let call = local::act::routed(
            &mut act_engine.borrow_mut(),
            &grammar,
            &request,
            &History::new(),
            local::act::Slots::ONE,
            |_| true,
            |_| Next::Continue,
        );
        act::decode_answer(&act::encode_answer(&call))
    });
    GateHost::with_act(reference(engine, true), act)
}

/// A Rhai literal for an argument value.
fn literal(value: &Value) -> String {
    match value {
        Value::Null => "()".into(),
        Value::String(s) => Value::String(s.clone()).to_string(),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(literal).collect::<Vec<_>>().join(", ")
        ),
        other => other.to_string(),
    }
}

/// The answer the shell gets for a decoded call.
pub fn answer(call: &Value) -> Answer {
    let args = &call["arguments"];
    let text = |key: &str| args[key].as_str().unwrap_or("").to_string();
    match call["name"].as_str().unwrap_or("") {
        "script" => Answer {
            script: text("code"),
            summary: "Runs a script.".into(),
        },
        "say" => Answer {
            script: String::new(),
            summary: text("text"),
        },
        "clarify" => Answer {
            script: String::new(),
            summary: text("question"),
        },
        name => {
            let values: Vec<String> = args
                .as_object()
                .map(|m| m.values().map(literal).collect())
                .unwrap_or_default();
            let call = format!("{name}({})", values.join(", "));
            let reads = super::registry::find(&super::registry::rows(80), name)
                .is_some_and(|row| row.undo == super::registry::Undo::Read);
            Answer {
                script: match reads {
                    true => format!("print({call});"),
                    false => format!("{call};"),
                },
                summary: format!("Calls {name}."),
            }
        }
    }
}

/// Script requests answered by a tagged arm.
pub fn tagged(tagging: Rc<Tagging>) -> Transport {
    Box::new(move |request: &ScriptRequest| {
        let mut turns = base_turns(&tagging.pack);
        turns.push((
            infer::chat::Role::User,
            user_text("", &request.request, tagging.no_think),
        ));
        if let Some((script, error)) = &request.repair {
            let previous = json!({"name": "script", "arguments": {"code": script}}).to_string();
            turns.push((infer::chat::Role::Assistant, previous));
            turns.push((
                infer::chat::Role::User,
                format!("That call was refused: {error}\nAnswer with a corrected call."),
            ));
        }
        let Decoded { call, .. } = decode(
            &mut tagging.engine.borrow_mut(),
            &tagging.grammar,
            &turns,
            tagging.seed,
        )
        .map_err(AnswerError::Local)?;
        let call = call.ok_or(AnswerError::CutOff)?;
        if tagging.m8script && call["name"] == "script" {
            // The tagged call chose a script; today's prompt and envelope write it.
            let request = ScriptRequest::decode(&request.encode())
                .ok_or_else(|| AnswerError::Server("malformed script request".into()))?;
            return local::script_with(
                &mut tagging.engine.borrow_mut(),
                &request,
                tagging.no_think,
                |_| Next::Continue,
            );
        }
        let answer = answer(&call);
        // As `script::answer` reads an empty script: the model answered in words,
        // which a refusal task counts as refusing.
        match answer.script.is_empty() {
            true => Err(AnswerError::NoScript(answer.summary)),
            false => Ok(answer),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_become_the_scripts_the_shell_runs() {
        let policy = json!({"name":"set_policy","arguments":{"name":"round-robin"}});
        assert_eq!(answer(&policy).script, "set_policy(\"round-robin\");");
        let cpus = json!({"name":"cpus","arguments":{}});
        assert_eq!(answer(&cpus).script, "print(cpus());");
        let say = json!({"name":"say","arguments":{"text":"Hi."}});
        assert_eq!(
            answer(&say),
            Answer {
                script: String::new(),
                summary: "Hi.".into()
            }
        );
        let code = json!({"name":"script","arguments":{"code":"print(1);"}});
        assert_eq!(answer(&code).script, "print(1);");
    }
}
