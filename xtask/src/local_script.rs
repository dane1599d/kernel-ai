//! The `local-script` boot case's outcome, worked out on the host: the shell
//! (`shellcore`) asks the generated model `infer::fixture::local_script` for a
//! script through the same engine, context cap, memory budget, sampler and JSON
//! shape the llm service uses (`llmproto::local::script`), and the same wire in
//! between ([`served`]), then checks the answer and refuses or proposes it
//! exactly as the guest's shell does. The harness types
//! the request at the terminal; the kernel then sends the same shell the same
//! request again, which llm answers with the prompt already in its cache. Every
//! engine tier must give the same two replies.

use crate::fluency::host::GateHost;
use infer::Next;
use infer::chat::{Budget, Engine, Step};
use llmproto::local;
use llmproto::script::{self, Answer, AnswerError, ScriptRequest};
use llmproto::text::printable;
use quant::Tier;
use shellcore::{DEFAULT_CONFIG, Shell};
use std::cell::RefCell;
use std::rc::Rc;

/// What the harness types at the terminal and the kernel sends again: plain
/// English, so the shell asks for a script.
pub const REQUEST: &str = "show me the cpus please";
/// The most characters of an error the llm service puts on the wire
/// (`MAX_ERROR_CHARS` in `user/src/bin/llm.rs`).
const SERVICE_ERROR_CHARS: usize = 160;

/// A script request answered on `engine` as the guest's shell gets it from the
/// llm service: the request through the wire format, so it is clipped the same,
/// and the answer or the error back through it, an error in one printable line
/// of at most 160 characters.
pub fn served(
    engine: &mut Engine<'_>,
    request: &ScriptRequest,
    on_step: impl FnMut(Step) -> Next,
) -> Result<Answer, AnswerError> {
    let wire = |error: &str| script::decode_error(&printable(error, SERVICE_ERROR_CHARS, 1));
    let Some(request) = ScriptRequest::decode(&request.encode()) else {
        return Err(wire("server malformed script request"));
    };
    match local::script(engine, &request, on_step) {
        Ok(answer) => script::decode_answer(&script::encode_answer(&answer))
            .ok_or_else(|| AnswerError::Server("llm service: malformed answer".into())),
        Err(error) => Err(wire(&script::encode_error(&error))),
    }
}

/// The shell's two replies on the wire (`err <why>`, say), and the script rounds
/// each took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub typed: String,
    pub again: String,
    pub rounds: [usize; 2],
}

/// The text the system prompts are made of, which the local-script model's
/// vocabulary is trained on so they take few tokens.
pub fn prompts() -> String {
    local::system_prompts()
}

/// The engine the llm service runs on the model in `bytes`, on `tier`.
pub fn engine(bytes: &'static [u8], tier: Tier) -> Result<Engine<'static>, String> {
    let budget = Budget {
        kv_bytes: kabi::model::KV_BYTES as usize,
        other_bytes: kabi::model::ENGINE_OTHER_BYTES as usize,
    };
    Engine::load(bytes, tier, kabi::model::CONTEXT_CAP, budget)
        .map_err(|e| format!("the local-script model: {e}"))
}

/// The two replies on `tier`, from one shell and one engine as in the guest.
fn outcome(bytes: &'static [u8], tier: Tier) -> Result<Outcome, String> {
    let engine = Rc::new(RefCell::new(engine(bytes, tier)?));
    let transport = Box::new(move |request: &ScriptRequest| {
        served(&mut engine.borrow_mut(), request, |_| Next::Continue)
    });
    let host = Rc::new(GateHost::new(transport));
    let mut shell = Shell::new(host.clone(), DEFAULT_CONFIG);
    let typed = shell.eval(REQUEST).to_wire();
    let first = host.answers.borrow().len();
    let again = shell.eval(REQUEST).to_wire();
    let second = host.answers.borrow().len() - first;
    Ok(Outcome {
        typed,
        again,
        rounds: [first, second],
    })
}

/// The guest's outcome on the model in `bytes`.
pub fn golden(bytes: &[u8]) -> Result<Outcome, String> {
    // The shell's host lives as long as the shell, so the engine borrows bytes
    // that do too: a copy kept for the rest of this run.
    let bytes: &'static [u8] = Box::leak(bytes.to_vec().into_boxed_slice());
    let mut first: Option<Outcome> = None;
    for tier in Tier::all() {
        let outcome = outcome(bytes, tier)?;
        match &first {
            None => first = Some(outcome),
            Some(seen) if *seen != outcome => {
                return Err(format!(
                    "the {} tier gives {outcome:?}, not {seen:?}",
                    tier.name()
                ));
            }
            Some(_) => {}
        }
    }
    first.ok_or_else(|| String::from("no engine tier"))
}

/// The console lines the terminal prints for the shell's reply `wire`: its lead
/// (`=` for ok, `!` for err, `?` for a confirmation) and each line in pieces.
pub fn reply_lines(wire: &str) -> Vec<String> {
    let (kind, rest) = wire.split_once(' ').unwrap_or((wire, ""));
    let (lead, body) = match kind {
        "ok" => ("=", rest),
        "err" => ("!", rest),
        "confirm" => ("?", rest.split_once(' ').map_or("", |(_, body)| body)),
        _ => ("!", "the shell sent a malformed reply"),
    };
    crate::local_ask::shown_lines(lead, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_refuses_the_models_script_after_checking_it_twice() {
        let bytes = infer::fixture::local_script(&prompts());
        let outcome = golden(&bytes).unwrap();
        println!("typed: {}\nagain: {}", outcome.typed, outcome.again);
        assert_eq!(golden(&bytes).unwrap(), outcome);
        // The script reached the shell's own checks, which refused it, and the
        // repair round did not save it.
        assert_eq!(outcome.rounds, [2, 2]);
        for reply in [&outcome.typed, &outcome.again] {
            assert!(
                reply.starts_with("err gave up after one repair: syntax error: "),
                "{reply}"
            );
        }
        let lines = reply_lines(&outcome.typed);
        assert_eq!(lines, [format!("! {}", &outcome.typed[4..])]);
    }

    #[test]
    fn replies_are_shown_with_their_lead() {
        assert_eq!(reply_lines("ok CPUs: 4"), ["= CPUs: 4"]);
        assert_eq!(reply_lines("err no"), ["! no"]);
        assert_eq!(
            reply_lines("confirm 3 script 3: reads cpus\n 1 | x"),
            ["? script 3: reads cpus", "?  1 | x"]
        );
    }
}
