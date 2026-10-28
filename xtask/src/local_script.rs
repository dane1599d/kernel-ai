//! The `local-script` boot case's outcome, worked out on the host: the shell
//! (`shellcore`) asks the generated model `infer::fixture::local_script` for a
//! tagged call (`act`, through `llmproto::local::act::routed`), and for a script
//! when no call comes back, through the same engine, context cap, memory budget, sampler and JSON
//! shape the llm service uses (`llmproto::local::script`), and the same wire in
//! between ([`served`]), then checks the answer and refuses or proposes it
//! exactly as the guest's shell does. The harness types
//! the request at the terminal; the kernel then sends the same shell the same
//! request again, which llm answers with the prompt already in its cache. Every
//! engine tier must give the same two replies.

use crate::fluency::host::GateHost;
use infer::Next;
use infer::chat::{Budget, Engine, Step};
use llmproto::act::{self, ActError, ActRequest, Call, History};
use llmproto::local;
use llmproto::script::{self, Answer, AnswerError, ScriptRequest};
use llmproto::text::printable;
use quant::Tier;
use shellcore::{DEFAULT_CONFIG, Host, Shell};
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

/// The engine the llm service runs on the model in `bytes`, on `tier`, with the
/// script system prompt in its cache as the service computes it after loading.
pub fn engine(bytes: &'static [u8], tier: Tier) -> Result<Engine<'static>, String> {
    let budget = Budget {
        kv_bytes: kabi::model::KV_BYTES as usize,
        other_bytes: kabi::model::ENGINE_OTHER_BYTES as usize,
    };
    let mut engine =
        Engine::load(bytes, tier, kabi::model::CONTEXT_CAP, budget).map_err(|e| e.to_string())?;
    local::warm(&mut engine, |_| Next::Continue).map_err(|e| local::describe(&e))?;
    Ok(engine)
}

/// The two replies on `tier`, from one shell and one engine as in the guest.
fn outcome(bytes: &'static [u8], tier: Tier) -> Result<Outcome, String> {
    let engine = engine(bytes, tier).map_err(|e| format!("the local-script model: {e}"))?;
    let engine = Rc::new(RefCell::new(engine));
    let scripts = Rc::clone(&engine);
    let transport = Box::new(move |request: &ScriptRequest| {
        served(&mut scripts.borrow_mut(), request, |_| Next::Continue)
    });
    let host = Rc::new(GateHost::new(transport));
    let grammar = local::act::grammar().map_err(|e| local::describe(&e))?;
    let acting: Rc<dyn Host> = Rc::new(Acting {
        gate: Rc::clone(&host),
        engine,
        grammar,
        history: RefCell::new(History::new()),
    });
    let mut shell = Shell::new(acting, DEFAULT_CONFIG);
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

/// The gate's stand-in OS, with `act` answered as the llm service answers it: the
/// tagged call on the same engine (`llmproto::local::act::routed`), through the
/// same wire, so a call that fails comes back as the guest's shell sees it. It
/// keeps conversation memory as llm does (`remember`, `new`), so a second request
/// carries the first turn as the guest's does.
struct Acting {
    gate: Rc<GateHost>,
    engine: Rc<RefCell<Engine<'static>>>,
    grammar: local::act::Grammar,
    history: RefCell<History>,
}

impl Host for Acting {
    fn now_ns(&self) -> u64 {
        self.gate.now_ns()
    }

    fn interrupted(&self) -> bool {
        self.gate.interrupted()
    }

    fn heap_bytes(&self) -> u64 {
        self.gate.heap_bytes()
    }

    fn metrics(&self) -> Result<kabi::Metrics, kabi::Error> {
        self.gate.metrics()
    }

    fn set_policy(&self, id: u64) -> Result<(), kabi::Error> {
        self.gate.set_policy(id)
    }

    fn ask(&self, prompt: &str) -> Result<String, String> {
        self.gate.ask(prompt)
    }

    fn script(&self, request: &ScriptRequest) -> Result<Answer, AnswerError> {
        self.gate.script(request)
    }

    fn act(&self, request: &ActRequest) -> Result<Call, ActError> {
        let Some(request) = ActRequest::decode(&request.encode()) else {
            return Err(ActError::NotACall);
        };
        let call = local::act::routed(
            &mut self.engine.borrow_mut(),
            &self.grammar,
            &request,
            &self.history.borrow(),
            // One slot here: the answers are the guest's whichever slot holds
            // which prompt; only the time differs.
            local::act::Slots::ONE,
            |_| true,
            |_| Next::Continue,
        );
        act::decode_answer(&act::encode_answer(&call))
    }

    /// As llm takes it: through the wire, and kept within its budget.
    fn remember(&self, line: &str, call: &str, result: &str) {
        let turn = act::Turn {
            line: line.to_string(),
            call: call.to_string(),
            result: result.to_string(),
        };
        if let Some(turn) = act::history::decode_remember(&act::history::encode_remember(&turn)) {
            local::act::remember(&self.engine.borrow(), &mut self.history.borrow_mut(), turn);
        }
    }

    fn forget(&self) {
        self.history.borrow_mut().clear();
    }

    fn audit(&self, text: &str) {
        self.gate.audit(text);
    }
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
    fn the_shell_answers_the_models_call_the_same_way_twice() {
        let bytes = infer::fixture::local_script(&prompts());
        let outcome = golden(&bytes).unwrap();
        println!("typed: {}\nagain: {}", outcome.typed, outcome.again);
        assert_eq!(golden(&bytes).unwrap(), outcome);
        // The generated model's tagged call is a read, run through the one door
        // with no script asked for, and a read again with the first turn in
        // history. The fixture model is random weights: which read it picks
        // follows the pack's bytes and the history, so the two need not match;
        // the same input twice must (above). A file read, which this guest,
        // with no kai-data disk, refuses in the shell's own words, shows `! `.
        assert_eq!(outcome.rounds, [0, 0]);
        for reply in [&outcome.typed, &outcome.again] {
            let lead = match reply.contains(shellcore::NO_FILES) {
                true => "! ",
                false => "= ",
            };
            assert!(reply_lines(reply)[0].starts_with(lead), "{reply}");
        }
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
