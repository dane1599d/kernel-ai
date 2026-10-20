//! The LLM service. It takes `ask <prompt>`, `explain <facts>` (see
//! `llmproto::explain`), `script ...` (see `llmproto::script`), `tune ...` (see
//! `llmproto::tune`, answered with the model's raw completion), `health` and
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
use llmproto::chat::{self, NO_MODEL};
use llmproto::script::{self, AnswerError, ScriptRequest};
use llmproto::text::printable;
use llmproto::tune::{self, TuneRequest};
use llmproto::{explain, http};
use user::{arg, args, println, sys};

user::entry!(main);

const MAX_ERROR_CHARS: usize = 160;
/// Bytes hashed between two 1 ms sleeps, so hashing a large model never runs into
/// the burst limit.
const HASH_PIECE: usize = 16 * 1024 * 1024;
const NO_SERVER: &str = "no LLM server configured (boot with llm=IP:PORT)";

/// The model the kernel mapped into this process.
struct Model {
    len: usize,
    /// FNV-1a of every byte, worked out on the first `health`.
    hash: Option<u64>,
}

impl Model {
    /// Maps the model: none when the kernel holds no model or this process may
    /// not map it, an error when the mapping failed.
    fn map() -> Result<Option<Model>, kabi::Error> {
        let len = match sys::model_map() {
            Ok(len) => usize::try_from(len).map_err(|_| kabi::Error::TooBig)?,
            Err(kabi::Error::NotFound | kabi::Error::Permission) => return Ok(None),
            Err(error) => return Err(error),
        };
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
}

fn main() -> i32 {
    let args = args();
    let server = remote::Server::from_args(&args);
    match &server {
        Some(server) => println!(
            "up (restart {}); server {}, model {}",
            arg(&args, "restart").unwrap_or("0"),
            server.host,
            server.model
        ),
        None => println!("up; {NO_SERVER}"),
    }
    let (model, local) = match Model::map() {
        Ok(Some(model)) => {
            let local = load(model.bytes());
            (Some(model), Some(local))
        }
        Ok(None) => (None, None),
        Err(error) => {
            println!("model: MODEL_MAP failed: {error:?}");
            let why = format!("the kernel holds a model this service could not map ({error:?})");
            (None, Some(Err(why)))
        }
    };
    let mut backends = Backends {
        server,
        model,
        local,
    };
    let mut request = vec![0u8; kabi::MAX_MESSAGE];
    let mut response = vec![0u8; http::MAX_RESPONSE];
    // Whether the script system prompt may be missing from the engine's cache:
    // at first, and after a request with another prompt.
    let mut cold = true;
    loop {
        let mut caller = 0;
        let waiting = match (cold, &mut backends.local) {
            (true, Some(Ok(local))) => warm(local, &mut request, &mut caller),
            _ => None,
        };
        cold = false;
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
        cold = matches!(text.split(' ').next(), Some("ask" | "explain" | "model"));
        let mut reply = match handle(&mut backends, text, &mut response) {
            Ok(text) => format!("ok {text}"),
            Err(error) => format!("err {}", printable(&error, MAX_ERROR_CHARS, 1)),
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
        (Some(Err(why)), None) => {
            format!("the model cannot run ({why}), and there is no LLM server")
        }
        (None, None) => String::from(NO_MODEL),
    };
    Err(script::encode_error(&AnswerError::Unavailable(why)))
}

/// The model's answer to a `tune` request, as it came: the shell checks it. The
/// local model decodes it under the answer's schema, a server under
/// `response_format`.
fn tune(backends: &mut Backends, text: &str, response: &mut [u8]) -> Result<String, String> {
    let request = TuneRequest::decode(text).ok_or("malformed tune request")?;
    let completion = match (&mut backends.local, &backends.server) {
        (Some(Ok(local)), _) => local.tune(&request)?,
        (_, Some(server)) => server.tune(&request, response)?,
        (Some(Err(why)), None) => return Err(format!("the model cannot run: {why}")),
        (None, None) => return Err(String::from(NO_MODEL)),
    };
    Ok(tune::encode_completion(&completion))
}

/// The server's health first, then the model's and the engine's; either side may
/// be missing, not both.
fn health(backends: &mut Backends, response: &mut [u8]) -> Result<String, String> {
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
        return local
            .map(|local| format!("{local}; {NO_SERVER}"))
            .ok_or_else(|| String::from(NO_SERVER));
    };
    match (server.health(response), local) {
        (Ok(server), Some(local)) => Ok(format!("{server}; {local}")),
        (Ok(server), None) => Ok(server),
        (Err(error), Some(local)) => Err(format!("{error}; {local}")),
        (Err(error), None) => Err(error),
    }
}
