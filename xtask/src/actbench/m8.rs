//! The M8 fluency tasks inside actbench (ADR 0010 S10-1): the gate's own
//! harness ([`crate::fluency::attempt`]: the shell, its checks, one repair and
//! the judge) with the model's answers coming from an arm. Arm `m8` is today's
//! prompt and envelope byte for byte, `m8-a0` the same without `/no_think`; a
//! tagged arm answers with its call, which becomes the script the shell gets: a
//! `script` call's code as it is, `say` and `clarify` as an answer in words, and
//! a row's call as a one-line script that makes it.

use super::{Decoded, Tagged, base_turns, decode, user_text};
use crate::fluency::host::Transport;
use crate::fluency::local::Shared;
use infer::Next;
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
        call.map(|call| answer(&call)).ok_or(AnswerError::CutOff)
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
