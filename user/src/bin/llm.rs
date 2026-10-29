//! The LLM service. It takes `ask <prompt>`, `explain <facts>` (see
//! `llmproto::explain`), `script ...` (see `llmproto::script`), `tune ...` (see
//! `llmproto::tune`, answered with the model's raw completion), `act ...` (see
//! `llmproto::act`, one tagged call), `remember ...` (a turn for its
//! conversation memory, `llmproto::act::history`), `new`, `health` and
//! `model` requests from the kernel terminal and the shell, and answers
//! `ok <text>` or `err <text>`.
//!
//! When the kernel holds a model from the `kai-model` disk and grants this service
//! `cap::MODEL`, it maps the model read-only at start and runs it with Kernel AI's
//! own engine ([`local`]): `ask`, `explain` and `script` are answered here, on this
//! CPU, a script decoded as the JSON envelope the shell reads. Otherwise, or when
//! the model cannot run, they go to the LLM server the kernel names
//! (`llm=IP:PORT`, [`remote`]), which is CI's test double. With neither, they say
//! how to set a model up (`llmproto::chat::NO_MODEL`).

#![no_std]
#![no_main]

extern crate alloc;

// A program's crate root sits in src/bin, so its modules are named by path.
#[path = "llm/kvfile.rs"]
mod kvfile;
#[path = "llm/local.rs"]
mod local;
#[path = "llm/remote.rs"]
mod remote;
#[path = "llm/workers.rs"]
mod workers;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use kabi::model::Fnv1a;
use llmproto::act::{ActError, ActRequest, History, history};
use llmproto::chat::Completion;
use llmproto::chat::{self, NO_MODEL};
use llmproto::health::{State, Status};
use llmproto::script::{self, AnswerError, ScriptRequest};
use llmproto::text::printable;
use llmproto::tune::{self, TuneRequest, replay};
use llmproto::{explain, http};
use user::{arg, args, println, sys};

user::entry!(main);

const MAX_ERROR_CHARS: usize = 160;
/// Bytes hashed between two 1 ms sleeps, so hashing a large model never runs into
/// the burst limit.
const HASH_PIECE: usize = 16 * 1024 * 1024;
/// How often MODEL_MAP is asked again while the model store fills the model.
const STORE_POLL_MS: u64 = 100;

/// The model the kernel mapped into this process.
struct Model {
    len: usize,
    /// FNV-1a of every byte, worked out on the first `health`.
    hash: Option<u64>,
}

impl Model {
    /// Maps the model: none when the kernel holds no model or this process may
    /// not map it, an error when the mapping failed. While the kernel waits for
    /// the model store to fill the model it reserved (P2 task 10), MODEL_MAP
    /// answers Busy, and this asks again: the kernel ends the wait itself, with
    /// the model or with NotFound when `fs` gives up, so there is no time limit.
    fn map() -> Result<Option<Model>, kabi::Error> {
        let mut waited = false;
        let len = loop {
            match sys::model_map() {
                Ok(len) => break usize::try_from(len).map_err(|_| kabi::Error::TooBig)?,
                Err(kabi::Error::Busy) => {
                    if !waited {
                        println!("model: waiting for the model store to fill the model");
                        waited = true;
                    }
                    sys::sleep_ms(STORE_POLL_MS);
                }
                Err(kabi::Error::NotFound | kabi::Error::Permission) => return Ok(None),
                Err(error) => return Err(error),
            }
        };
        if waited {
            println!("model: the model store filled the model");
        }
        println!(
            "model: {len} bytes mapped read-only at {:#x}",
            kabi::layout::MODEL_BASE
        );
        Ok(Some(Model { len, hash: None }))
    }

    fn bytes(&self) -> &'static [u8] {
        // SAFETY: MODEL_MAP mapped `len` readable bytes at MODEL_BASE for the rest of
        // this process's life, and nothing writes or unmaps them.
        unsafe { core::slice::from_raw_parts(kabi::layout::MODEL_BASE as *const u8, self.len) }
    }

    /// `model <len> bytes fnv1a64 <hash>`, as `health` reports it.
    fn describe(&mut self) -> String {
        let hash = match self.hash {
            Some(hash) => hash,
            None => {
                let mut hash = Fnv1a::new();
                for piece in self.bytes().chunks(HASH_PIECE) {
                    hash.update(piece);
                    sys::sleep_ms(1);
                }
                *self.hash.insert(hash.finish())
            }
        };
        format!("model {} bytes fnv1a64 {hash:016x}", self.len)
    }
}

/// What can answer: the server, the mapped model and the engine running it (or why
/// it cannot), in any combination.
struct Backends {
    server: Option<remote::Server>,
    model: Option<Model>,
    local: Option<Result<local::Local, String>>,
    /// An `llmreplay` file on the model disk instead of a model.
    replay: Option<&'static [u8]>,
    /// `tune` requests traced so far.
    traced: u64,
    /// The conversation a server is sent, costed by [`server_cost`]. The local
    /// backend keeps its own, costed by its tokenizer; `remember` and `new` keep
    /// both in step, and a reply reports the one that answers.
    history: History,
    /// A slot's prompt is not cached yet: `health` says `warming`.
    warming: bool,
}

/// What a turn costs a server's history: about four bytes a token, as no
/// tokenizer is at hand for a server's model.
fn server_cost(turn: &history::Turn) -> usize {
    turn.messages()
        .iter()
        .map(|m| m.content.len())
        .sum::<usize>()
        .div_ceil(4)
}

fn main() -> i32 {
    let args = args();
    let server = remote::Server::from_args(&args);
    let restart = arg(&args, "restart").unwrap_or("0");
    match &server {
        Some(server) => println!(
            "up (restart {restart}); server {}, model {}",
            server.host, server.model
        ),
        None => println!("up (restart {restart})"),
    }
    let (model, local, replay) = match Model::map() {
        Ok(Some(model)) if model.bytes().starts_with(kabi::model::REPLAY_MAGIC) => {
            println!("llmreplay: answering recorded tune prompts only");
            let bytes = model.bytes();
            (None, None, Some(bytes))
        }
        Ok(Some(model)) => {
            let mut local = load(model.bytes());
            let sha = arg(&args, "model_sha256").and_then(kabi::model::sha256_from_hex);
            if let (Ok(local), Some(sha)) = (&mut local, sha) {
                local.set_sha256(sha);
            }
            (Some(model), Some(local), None)
        }
        Ok(None) => (None, None, None),
        Err(error) => {
            println!("model: MODEL_MAP failed: {error:?}");
            let why = format!("the kernel holds a model this service could not map ({error:?})");
            (None, Some(Err(why)), None)
        }
    };
    let mut backends = Backends {
        server,
        model,
        local,
        replay,
        traced: 0,
        history: History::new(),
        warming: false,
    };
    let mut request = vec![0u8; kabi::MAX_MESSAGE];
    let mut response = vec![0u8; http::MAX_RESPONSE];
    // Whether the script system prompt may be missing from the engine's cache:
    // at first, and after a request with another prompt.
    let mut cold = true;
    // Whether the `act` prefix changed (a remembered turn, `new`): compacted,
    // that is computed again, while no request waits.
    let mut stale = true;
    // Interim (S10-3): the slots' prefixes from last boot's KV files; a slot
    // that did not come back is saved once its warm-up finishes.
    let (mut save_act, mut save_script) = match &mut backends.local {
        Some(Ok(local)) => {
            let (act, script) = local.restore();
            (!act, !script)
        }
        _ => (false, false),
    };
    loop {
        let mut caller = 0;
        let local = match &mut backends.local {
            Some(Ok(local)) => Some(local),
            _ => None,
        };
        // Each slot is warmed on its own while no request waits: the act prefix
        // at first and when history changed, the script prompt at first and after
        // another prompt took the script slot. A warm-up a request cut short is
        // finished at the next idle turn.
        let waiting = match local {
            Some(local) if stale => {
                let came = warm_act(local, &mut request, &mut caller);
                stale = came.is_some();
                if !stale && core::mem::take(&mut save_act) {
                    local.save(local::SLOTS.act);
                }
                came
            }
            Some(local) if cold => {
                let came = warm(local, &mut request, &mut caller);
                cold = came.is_some();
                if !cold && core::mem::take(&mut save_script) {
                    local.save(local::SLOTS.script);
                }
                came
            }
            _ => None,
        };
        if waiting.is_none() && local_warmable(&backends, stale, cold) {
            continue;
        }
        let len = match waiting.map_or_else(|| sys::svc_recv(&mut request, 0, &mut caller), Ok) {
            Ok(len) => len.min(request.len()),
            Err(kabi::Error::TimedOut) => continue,
            Err(error) => {
                println!("svc_recv failed: {error:?}");
                sys::sleep_ms(100);
                continue;
            }
        };
        let text = core::str::from_utf8(&request[..len]).unwrap_or("");
        let command = text.split([' ', '\n']).next();
        cold |= matches!(command, Some("ask" | "explain" | "model" | "tune"));
        stale |= matches!(command, Some("remember" | "new"));
        backends.warming = local_warmable(&backends, stale, cold);
        let mut reply = match command {
            // `warm`: whether both slots hold their prompts (S02-2 waits for it).
            Some("warm") => match local_warmable(&backends, stale, cold) {
                true => String::from("ok no"),
                false => String::from("ok yes"),
            },
            _ => match handle(&mut backends, text, &mut response) {
                Ok(text) => format!("ok {text}"),
                Err(error) => format!("err {}", printable(&error, MAX_ERROR_CHARS, 1)),
            },
        };
        // The character cap can still exceed the kernel's message size in bytes.
        reply.truncate(reply.floor_char_boundary(kabi::MAX_MESSAGE));
        if let Err(error) = sys::svc_reply(reply.as_bytes()) {
            println!("svc_reply failed: {error:?}");
        }
    }
}

/// Starts the engine on the mapped model, on one worker per CPU, logging how it
/// went.
fn load(bytes: &'static [u8]) -> Result<local::Local, String> {
    let started = sys::time_ns();
    let loaded = local::Local::load(bytes, workers::start());
    let ms = (sys::time_ns() - started) / 1_000_000;
    match &loaded {
        Ok(engine) => println!("local: {} ready in {ms} ms", engine.describe()),
        Err(why) => println!("local: the model cannot run: {why}"),
    }
    // Loading may be a long burst; a sleep ends it before the first request.
    sys::sleep_ms(1);
    loaded
}

/// Whether a warm-up is still to do on a model that runs.
fn local_warmable(backends: &Backends, stale: bool, cold: bool) -> bool {
    matches!(backends.local, Some(Ok(_))) && (stale || cold)
}

/// Compacts history: the `act` prefix into the cache, as [`warm`] does the script
/// prompt.
fn warm_act(local: &mut local::Local, request: &mut [u8], caller: &mut u64) -> Option<usize> {
    let mut came = None;
    if let Err(why) = local.warm_act(|| {
        came = sys::svc_recv(request, 1, caller).ok();
        came.is_some()
    }) {
        println!("local: the act prefix was not cached: {why}");
    }
    came
}

/// Warms `local` for scripts until done or until a request comes, which is then
/// in `request`: its length is returned. Waiting for one takes 1 ms, so the
/// check ends the burst as the sleep it replaces would.
fn warm(local: &mut local::Local, request: &mut [u8], caller: &mut u64) -> Option<usize> {
    let started = sys::time_ns();
    let mut came = None;
    let warmed = local.warm(|| {
        came = sys::svc_recv(request, 1, caller).ok();
        came.is_some()
    });
    let ms = (sys::time_ns() - started) / 1_000_000;
    match (warmed, came) {
        (Err(why), _) => println!("local: the script prompt was not cached: {why}"),
        (Ok(()), None) => println!("local: script prompt cached in {ms} ms"),
        (Ok(()), Some(_)) => {}
    }
    came
}

fn handle(backends: &mut Backends, text: &str, response: &mut [u8]) -> Result<String, String> {
    let (command, rest) = text.split_once(' ').unwrap_or((text, ""));
    match command {
        "health" => health(backends, response),
        "model" if rest.is_empty() => match (&backends.model, &mut backends.local) {
            (Some(model), Some(Ok(local))) => Ok(local.report(model.len)),
            (_, Some(Err(why))) => Err(format!("the model cannot run: {why}")),
            _ => Err(String::from(NO_MODEL)),
        },
        "ask" if !rest.trim().is_empty() => {
            let question = rest.trim();
            answer(
                backends,
                &chat::ask(question),
                chat::answer,
                |server, response| server.ask(question, response),
                response,
            )
        }
        "explain" if rest.len() > explain::MAX_FACTS => Err(String::from("the facts are too long")),
        "explain" if !rest.trim().is_empty() => answer(
            backends,
            &explain::request(rest),
            explain::answer,
            |server, response| server.explain(rest, response),
            response,
        ),
        "script" => script(backends, text, response),
        "act" => act(backends, text, response),
        "remember" => {
            let turn = history::decode_remember(text).ok_or("malformed remember request")?;
            let cost = server_cost(&turn);
            let dropped = backends
                .history
                .push(turn.clone(), cost, history::HISTORY_MAX);
            match &mut backends.local {
                Some(Ok(local)) => Ok(local.remember(turn)),
                _ => Ok(format!(
                    "{} turns, about {} of {} tokens{}",
                    backends.history.len(),
                    backends.history.tokens(),
                    history::HISTORY_MAX,
                    if dropped {
                        " (the older half went)"
                    } else {
                        ""
                    }
                )),
            }
        }
        "new" if rest.is_empty() => {
            backends.history.clear();
            if let Some(Ok(local)) = &mut backends.local {
                local.forget();
            }
            Ok(String::from("conversation cleared"))
        }
        "tune" => tune(backends, text, response),
        _ => Err(format!("unknown request {command:?}")),
    }
}

/// Answers `request` on the local model when it runs, through `check`; otherwise
/// on the server with `remote`. Scripts pick their backend the same way.
fn answer(
    backends: &mut Backends,
    request: &chat::Request,
    check: fn(&chat::Completion) -> Result<String, String>,
    remote: impl FnOnce(&remote::Server, &mut [u8]) -> Result<String, String>,
    response: &mut [u8],
) -> Result<String, String> {
    match (&mut backends.local, &backends.server) {
        (Some(Ok(local)), _) => local.complete(request).and_then(|done| check(&done)),
        (_, Some(server)) => remote(server, response),
        (Some(Err(why)), None) => Err(format!("the model cannot run: {why}")),
        (None, None) => Err(String::from(NO_MODEL)),
    }
}

/// A script from the local model when it runs, as [`answer`] picks; the answer
/// goes back framed by length, and the shell checks it again before showing it.
fn script(backends: &mut Backends, text: &str, response: &mut [u8]) -> Result<String, String> {
    let request = ScriptRequest::decode(text).ok_or("server malformed script request")?;
    let why = match (&mut backends.local, &backends.server) {
        (Some(Ok(local)), _) => {
            return local
                .script(&request)
                .map(|answer| script::encode_answer(&answer))
                .map_err(|error| script::encode_error(&error));
        }
        (_, Some(server)) => return server.script(&request, response),
        (Some(Err(why)), None) => format!("the model cannot run ({why})"),
        (None, None) => String::from(NO_MODEL),
    };
    Err(script::encode_error(&AnswerError::Unavailable(why)))
}

/// One tagged call: the local model's decoder when it runs, else a server's
/// answer to the same prompt, read only if it is exactly a call.
fn act(backends: &mut Backends, text: &str, response: &mut [u8]) -> Result<String, String> {
    let request = ActRequest::decode(text).ok_or("malformed act request")?;
    let answer = match (&mut backends.local, &backends.server) {
        (Some(Ok(local)), _) => local.act(&request),
        (_, Some(server)) => server.act(&request, &backends.history, response),
        (Some(Err(why)), None) => Err(ActError::Unavailable(format!(
            "the model cannot run ({why})"
        ))),
        (None, None) => Err(ActError::Unavailable(String::from(NO_MODEL))),
    };
    Ok(llmproto::act::encode_answer(&answer))
}

/// The model's answer to a `tune` request, as it came: the shell checks it. The
/// local model decodes it under the answer's schema, a server under
/// `response_format`.
fn tune(backends: &mut Backends, text: &str, response: &mut [u8]) -> Result<String, String> {
    let request = TuneRequest::decode(text).ok_or("malformed tune request")?;
    backends.traced += 1;
    let seq = backends.traced;
    for line in replay::trace("request", seq, &request.encode()) {
        println!("{line}");
    }
    let answer = tune_answer(backends, &request, response).map(|c| tune::encode_completion(&c));
    let traced = answer.as_deref().unwrap_or("");
    for line in replay::trace("answer", seq, traced) {
        println!("{line}");
    }
    answer
}

fn tune_answer(
    backends: &mut Backends,
    request: &TuneRequest,
    response: &mut [u8],
) -> Result<Completion, String> {
    if let Some(file) = backends.replay {
        return replay::answer(file, request).map_err(|error| error.describe());
    }
    let request = request.clone();
    let completion = match (&mut backends.local, &backends.server) {
        (Some(Ok(local)), _) => local.tune(&request)?,
        (_, Some(server)) => server.tune(&request, response)?,
        (Some(Err(why)), None) => return Err(format!("the model cannot run: {why}")),
        (None, None) => return Err(String::from(NO_MODEL)),
    };
    Ok(completion)
}

/// The server's health first, then the model's and the engine's; either side may
/// be missing, not both. A reply ends with the model's state and speed for the
/// Monitor app (`llmproto::health`).
fn health(backends: &mut Backends, response: &mut [u8]) -> Result<String, String> {
    let status = Status {
        state: match (&backends.local, &backends.server) {
            (Some(Ok(_)), _) if backends.warming => State::Warming,
            (Some(Ok(_)), _) => State::Ready,
            (Some(Err(_)), _) => State::Failed,
            (None, Some(_)) => State::Remote,
            (None, None) => State::NoModel,
        },
        tps10: match &backends.local {
            Some(Ok(local)) => local.tps10(),
            _ => 0,
        },
    };
    described(backends, response).map(|text| format!("{text}; {}", status.line()))
}

/// [`health`]'s words, before the status.
fn described(backends: &mut Backends, response: &mut [u8]) -> Result<String, String> {
    let mut local: Vec<String> = Vec::new();
    if let Some(model) = &mut backends.model {
        local.push(model.describe());
    }
    match &backends.local {
        Some(Ok(engine)) => local.push(engine.describe()),
        Some(Err(why)) => local.push(format!("the model cannot run: {why}")),
        None => {}
    }
    let local = (!local.is_empty()).then(|| local.join("; "));
    let Some(server) = &backends.server else {
        return local.ok_or_else(|| String::from(NO_MODEL));
    };
    match (server.health(response), local) {
        (Ok(server), Some(local)) => Ok(format!("{server}; {local}")),
        (Ok(server), None) => Ok(server),
        (Err(error), Some(local)) => Err(format!("{error}; {local}")),
        (Err(error), None) => Err(error),
    }
}
