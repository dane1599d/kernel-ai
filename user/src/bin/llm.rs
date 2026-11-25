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
#[path = "llm/apps.rs"]
mod apps;
#[path = "llm/kvfile.rs"]
mod kvfile;
#[path = "llm/local.rs"]
mod local;
#[path = "llm/remote.rs"]
mod remote;
#[path = "llm/workers.rs"]
mod workers;

use agentsched::Now;
use agentsched::jobs::{self, Caller, Jobs};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use kabi::handle::{Handle, RecvInfo};
use kabi::model::Fnv1a;
use llmproto::act::{ActError, ActRequest, History, history};
use llmproto::chat::Completion;
use llmproto::chat::{self, NO_MODEL};
use llmproto::health::{State, Status};
use llmproto::query::{self, QueryRequest};
use llmproto::script::{self, AnswerError, Lang, ScriptRequest};
use llmproto::text::printable;
use llmproto::tune::{self, TuneRequest, replay};
use llmproto::{appjob, explain, http};
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
    /// Agents' requests (ADR 0013 section 2, F3 slice 2a): started by a route's
    /// badge, run in idle turns.
    jobs: Jobs,
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
    // A test boot's `cold_ms=N` (the `desk-warming` case): `warm` says no for N
    // ms after llm starts, as a local model filling its slots would.
    let cold_until = arg(&args, "cold_ms")
        .and_then(|ms| ms.parse::<u64>().ok())
        .map(|ms| sys::time_ns().saturating_add(ms.saturating_mul(1_000_000)));
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
        jobs: Jobs::new(agentsched::Config::default(), restart.parse().unwrap_or(0)),
    };
    let mut request = vec![0u8; kabi::MAX_MESSAGE];
    let mut response = vec![0u8; http::MAX_RESPONSE];
    // App rounds the shell starts and comes back for (`llmproto::appjob`).
    let mut jobs = appjob::Jobs::new(restart.parse().unwrap_or(0));
    // Whether the script system prompt may be missing from the engine's cache:
    // at first, and after a request with another prompt.
    let mut cold = true;
    // The prompt the script slot keeps warm: Rhai's, or the `c-1` card's
    // while the person is at `c>` (`warm-lang`).
    let mut warm_lang = Lang::Rhai;
    // Whether the `act` prefix changed (a remembered turn, `new`): compacted,
    // that is computed again, while no request waits.
    let mut stale = true;
    // Interim (S10-3): the slots' prefixes from last boot's KV files. The
    // script slot that did not come back is saved once its warm-up finishes;
    // the act slot's file is written whenever its prefix, with no
    // conversation in it, differs from the file (`Local::act_file_due`).
    let mut save_script = match &mut backends.local {
        Some(Ok(local)) => !local.restore().1,
        _ => false,
    };
    loop {
        let mut caller = RecvInfo::default();
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
                if !stale && local.act_file_due() {
                    local.save(local::SLOTS.act);
                }
                came
            }
            Some(local) if cold => {
                let came = warm(local, warm_lang, &mut request, &mut caller);
                cold = came.is_some();
                // The saved prefix is the Rhai prompt's.
                if !cold && warm_lang == Lang::Rhai && core::mem::take(&mut save_script) {
                    local.save(local::SLOTS.script);
                }
                came
            }
            _ => None,
        };
        if waiting.is_none() && local_warmable(&backends, stale, cold) {
            continue;
        }
        // Both slots are warm and no request waits: an agent's job may run, and gives
        // way to the next request that comes.
        let waiting = match waiting {
            None => run_agent_job(
                &mut backends,
                &mut request,
                &mut caller,
                &mut cold,
                &mut stale,
            ),
            came => came,
        };
        let len = match waiting.map_or_else(|| recv(&mut request, 0, &mut caller), Ok) {
            Ok(len) => len.min(request.len()),
            Err(kabi::Error::TimedOut) => continue,
            Err(error) => {
                println!("recv_h failed: {error:?}");
                sys::sleep_ms(100);
                continue;
            }
        };
        let text = core::str::from_utf8(&request[..len]).unwrap_or("");
        let command = text.split([' ', '\n']).next();
        // An agent's badge may send only `agent-*`: the person's conversation, app
        // rounds and prompts are theirs (and badge 0's services').
        if !jobs::may_serve(caller.badge, text) {
            if let Err(error) = sys::svc_reply(jobs::AGENTS_USE_JOBS.as_bytes()) {
                println!("svc_reply failed: {error:?}");
            }
            continue;
        }
        // An agent's job: queued here, answered at once, run in an idle turn.
        if command.is_some_and(|c| c.starts_with("agent-")) {
            let from = Caller {
                pid: caller.caller_pid,
                badge: caller.badge,
            };
            let mut reply = agent_command(&mut backends, from, text);
            reply.truncate(reply.floor_char_boundary(kabi::MAX_MESSAGE));
            if let Err(error) = sys::svc_reply(reply.as_bytes()) {
                println!("svc_reply failed: {error:?}");
            }
            continue;
        }
        // A request with another prompt than the warm one takes the script
        // slot: a C program's card, unless `c>` keeps it warm, and every other.
        cold |= match command {
            Some("script") => warm_lang != Lang::Rhai,
            Some("program") => warm_lang != Lang::C,
            Some("ask" | "explain" | "model" | "tune" | "query" | "app" | "app-start") => true,
            _ => false,
        };
        stale |= matches!(command, Some("remember" | "new"));
        backends.warming = local_warmable(&backends, stale, cold);
        let rest = text.split_once(' ').map_or("", |(_, rest)| rest);
        let mut job = None;
        let mut reply = match command {
            // A round is a job on the local engine, and on a server too, so the
            // build goes on in the background and its install card asks for
            // itself, as on the shipped system.
            Some("app-start") => match jobs.start(rest, runs_jobs(&backends)) {
                appjob::Start::Run(ticket, asked) => {
                    job = Some((ticket, asked));
                    format!("ok {ticket}")
                }
                appjob::Start::Reply(reply) => reply,
            },
            Some("app-take") => jobs.take(rest),
            // `warm-lang c|rhai`: the shell at `c>` and back at `kai>`.
            Some("warm-lang") => {
                let lang = match rest.trim() {
                    "c" => Some(Lang::C),
                    "rhai" => Some(Lang::Rhai),
                    _ => None,
                };
                match lang {
                    Some(lang) => {
                        cold |= lang != warm_lang;
                        warm_lang = lang;
                        backends.warming = local_warmable(&backends, stale, cold);
                        String::from("ok")
                    }
                    None => format!("err warm-lang takes c or rhai, not {:?}", rest.trim()),
                }
            }
            Some("app-stop") => jobs.stop(rest),
            // `warm`: whether both slots hold their prompts (S02-2 waits for it).
            Some("warm") if cold_until.is_some_and(|until| sys::time_ns() < until) => {
                String::from("ok no")
            }
            Some("warm") => match local_warmable(&backends, stale, cold) {
                true => String::from("ok no"),
                false => String::from("ok yes"),
            },
            _ => match handle(&mut backends, text, &mut response) {
                Ok(text) => format!("ok {text}"),
                Err(error) => format!("err {}", script::error_line(&error, MAX_ERROR_CHARS)),
            },
        };
        // The character cap can still exceed the kernel's message size in bytes.
        reply.truncate(reply.floor_char_boundary(kabi::MAX_MESSAGE));
        if let Err(error) = sys::svc_reply(reply.as_bytes()) {
            println!("svc_reply failed: {error:?}");
        }
        if let Some((ticket, asked)) = job {
            match (&mut backends.local, &backends.server) {
                (Some(Ok(local)), _) => run_job(local, &mut jobs, ticket, &asked, &mut request),
                (_, Some(server)) => {
                    run_server_job(server, &mut jobs, ticket, &asked, &mut response)
                }
                _ => {}
            }
        }
    }
}

/// Whether `app-start` runs a round as a job here: on the local engine when
/// it runs, else on a server, as [`script`] picks the backend.
fn runs_jobs(backends: &Backends) -> bool {
    matches!(backends.local, Some(Ok(_))) || backends.server.is_some()
}

/// App job `ticket`'s round on the server, kept for `app-take`. The request
/// blocks llm until the server answers: the shell's `app-take` that comes
/// meanwhile times out and asks again.
fn run_server_job(
    server: &remote::Server,
    jobs: &mut appjob::Jobs,
    ticket: u64,
    asked: &ScriptRequest,
    response: &mut [u8],
) {
    let reply = match server.script(asked, response) {
        Ok(answer) => format!("ok {answer}"),
        Err(error) => format!("err {}", printable(&error, MAX_ERROR_CHARS, 1)),
    };
    jobs.finish(ticket, reply);
}

/// The next request, with who sent it and its route's badge (`RECV_H`): the badge
/// is what names an agent, never the pid. llm takes no handles, so any a request
/// moved are closed at once.
fn recv(request: &mut [u8], timeout_ms: u64, info: &mut RecvInfo) -> Result<usize, kabi::Error> {
    let len = sys::recv_h(request, timeout_ms, info)?;
    let count = (info.handle_count as usize).min(info.handles.len());
    for &handle in &info.handles[..count] {
        let _ = sys::handle_close(Handle(handle));
    }
    Ok(len)
}

/// The time, as an agent's day counts it: the host's date when the firmware gave
/// one (days since 1970 UTC), else day 0 for the whole boot.
fn now() -> Now {
    Now {
        ms: sys::time_ns() / 1_000_000,
        day: sys::clock_now().map_or(0, |secs| (secs / 86_400) as u32),
    }
}

/// Answers an `agent-*` command from the route with `badge` (F3 slice 2a). Its
/// prompt's tokens are estimated from its bytes, for the scheduler's checks; what
/// it really took is charged when it has run.
fn agent_command(backends: &mut Backends, caller: Caller, text: &str) -> String {
    let command = match jobs::parse(text) {
        Ok(Some(command)) => command,
        Ok(None) => return format!("err unknown request {text:?}"),
        Err(why) => return format!("err {}", printable(&why, MAX_ERROR_CHARS, 1)),
    };
    let (prompt, max_new) = match command {
        jobs::Command::Start { kind, request } => (
            u32::try_from(request.len() / 3 + 1).unwrap_or(u32::MAX),
            match kind {
                jobs::Kind::Act => AGENT_ACT_TOKENS,
                jobs::Kind::Ask => AGENT_ASK_TOKENS,
                jobs::Kind::Script => AGENT_SCRIPT_TOKENS,
            },
        ),
        _ => (0, 0),
    };
    backends
        .jobs
        .handle(caller, command, prompt, max_new, now())
}

/// The most an agent's `act`, `ask` and `script` answers are counted at before they
/// run.
const AGENT_ACT_TOKENS: u32 = 256;
const AGENT_ASK_TOKENS: u32 = 512;
const AGENT_SCRIPT_TOKENS: u32 = 1024;
/// How often an agent's job looks for a request that came: the person waits at
/// most this, and the step in flight.
const AGENT_POLL_NS: u64 = 50_000_000;

/// Runs the next agent job, whole, while no request waits (F3 slice 2a). On the
/// local model it asks every [`AGENT_POLL_NS`] whether a request came; one that did
/// stops it, is answered next, and the job starts again from its prompt in a later
/// idle turn, exactly (ADR 0013 section 2). `Some(len)` when a request came.
fn run_agent_job(
    backends: &mut Backends,
    request: &mut [u8],
    caller: &mut RecvInfo,
    cold: &mut bool,
    stale: &mut bool,
) -> Option<usize> {
    let (ticket, job) = backends.jobs.next_job()?;
    let (kind, queued) = (job.kind, job.queued?);
    let text = format!("{} {}", kind.word(), job.request);
    let mut came = None;
    let mut polled = 0;
    let mut stop = || {
        let at = sys::time_ns();
        if came.is_none() && at.saturating_sub(polled) >= AGENT_POLL_NS {
            polled = at;
            request_came(request, caller, &mut came);
        }
        came.is_some()
    };
    let local = matches!(backends.local, Some(Ok(_)));
    let mut response = vec![0u8; http::MAX_RESPONSE];
    let answer = match kind {
        jobs::Kind::Script => agent_script(backends, &text, &mut response, &mut stop),
        jobs::Kind::Ask => agent_ask(backends, &text, &mut response, &mut stop),
        jobs::Kind::Act => agent_act(backends, &text, &mut response, &mut stop),
    };
    // The script slot held the agent's prompt: it is warmed again first. An agent's
    // call also took the act slot, with no conversation: the person's act prefix and
    // history are computed again before their next request.
    *cold |= local;
    *stale |= local && kind == jobs::Kind::Act;
    if came.is_some() {
        return came;
    }
    let reply = match answer {
        Ok(text) => format!("ok {text}"),
        Err(error) => format!("err {}", printable(&error, MAX_ERROR_CHARS, 1)),
    };
    if let Some(Ok(local)) = &backends.local {
        backends.jobs.charge(ticket, local.used(), now());
    }
    backends.jobs.finish(queued, reply);
    None
}

/// An agent's `act` job: one tagged call, decoded with no conversation, never the
/// person's (on a server too), with `stop` on the local model.
fn agent_act(
    backends: &mut Backends,
    text: &str,
    response: &mut [u8],
    stop: &mut dyn FnMut() -> bool,
) -> Result<String, String> {
    let request = ActRequest::decode(text).ok_or("malformed act request")?;
    let answer = match (&mut backends.local, &backends.server) {
        (Some(Ok(local)), _) => local.act_until(&request, local::Whose::Agent, stop),
        (_, Some(server)) => server.act(&request, &History::new(), response),
        (Some(Err(why)), None) => Err(ActError::Unavailable(format!(
            "the model cannot run ({why})"
        ))),
        (None, None) => Err(ActError::Unavailable(String::from(NO_MODEL))),
    };
    Ok(llmproto::act::encode_answer(&answer))
}

/// An agent's `script` job: [`script`], with `stop` on the local model.
fn agent_script(
    backends: &mut Backends,
    text: &str,
    response: &mut [u8],
    stop: &mut dyn FnMut() -> bool,
) -> Result<String, String> {
    let request = ScriptRequest::decode(text).ok_or("malformed script request")?;
    match &mut backends.local {
        Some(Ok(local)) => local
            .script_until(&request, stop)
            .map(|answer| script::encode_answer(&answer))
            .map_err(|error| script::encode_error(&error)),
        _ => script(backends, text, response),
    }
}

/// An agent's `ask` job: the answer, with `stop` on the local model.
fn agent_ask(
    backends: &mut Backends,
    text: &str,
    response: &mut [u8],
    stop: &mut dyn FnMut() -> bool,
) -> Result<String, String> {
    let question = text.split_once(' ').map_or("", |(_, rest)| rest).trim();
    match &mut backends.local {
        Some(Ok(local)) => local
            .complete_until(&chat::ask(question), stop)
            .and_then(|done| chat::answer(&done)),
        _ => handle(backends, text, response),
    }
}

/// App job `ticket`'s round on the local engine, answering each request that
/// comes meanwhile as [`appjob::Jobs::while_running`] says, and keeping its
/// reply for `app-take`.
fn run_job(
    local: &mut local::Local,
    jobs: &mut appjob::Jobs,
    ticket: u64,
    asked: &ScriptRequest,
    request: &mut [u8],
) {
    let mut caller = RecvInfo::default();
    // 1 ms: a timeout of 0 is the kernel's longest wait (60 s), which held
    // the round at each check until a request came, so it never finished.
    let answer = local.script_until(asked, || {
        let Ok(len) = recv(request, 1, &mut caller) else {
            return false;
        };
        let text = core::str::from_utf8(&request[..len.min(request.len())]).unwrap_or("");
        // An agent may not stop, take or read the person's app round.
        if !jobs::may_serve(caller.badge, text) || text.starts_with("agent-") {
            let refusal: &[u8] = match text.starts_with("agent-") {
                true => b"busy Kai is writing an app",
                false => jobs::AGENTS_USE_JOBS.as_bytes(),
            };
            if let Err(error) = sys::svc_reply(refusal) {
                println!("svc_reply failed: {error:?}");
            }
            return false;
        }
        let (mut reply, stop) = jobs.while_running(text);
        reply.truncate(reply.floor_char_boundary(kabi::MAX_MESSAGE));
        if let Err(error) = sys::svc_reply(reply.as_bytes()) {
            println!("svc_reply failed: {error:?}");
        }
        stop
    });
    let reply = match answer {
        Ok(answer) => format!("ok {}", script::encode_answer(&answer)),
        Err(error) => format!(
            "err {}",
            script::error_line(&script::encode_error(&error), MAX_ERROR_CHARS)
        ),
    };
    jobs.finish(ticket, reply);
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
fn warm_act(local: &mut local::Local, request: &mut [u8], caller: &mut RecvInfo) -> Option<usize> {
    let mut came = None;
    if let Err(why) = local.warm_act(|| request_came(request, caller, &mut came)) {
        println!("local: the act prefix was not cached: {why}");
    }
    came
}

/// Whether a request came while a warm-up computes, which then stops and leaves
/// it in `request` (its length in `came`). A `warm` ask is no such request: it
/// is answered at once, "ok no", and the warm-up goes on. So asking whether llm
/// is warm, as the shell does for the panel, never cuts a warm-up short or
/// changes what llm computes.
fn request_came(request: &mut [u8], caller: &mut RecvInfo, came: &mut Option<usize>) -> bool {
    *came = recv(request, 1, caller).ok();
    let Some(len) = *came else {
        return false;
    };
    if request.get(..len.min(request.len())) != Some(b"warm") {
        return true;
    }
    if let Err(error) = sys::svc_reply(b"ok no") {
        println!("svc_reply failed: {error:?}");
    }
    *came = None;
    false
}

/// Warms `local` for scripts until done or until a request comes, which is then
/// in `request`: its length is returned. Waiting for one takes 1 ms, so the
/// check ends the burst as the sleep it replaces would.
fn warm(
    local: &mut local::Local,
    lang: Lang,
    request: &mut [u8],
    caller: &mut RecvInfo,
) -> Option<usize> {
    let started = sys::time_ns();
    let mut came = None;
    let warmed = local.warm(lang, || request_came(request, caller, &mut came));
    let ms = (sys::time_ns() - started) / 1_000_000;
    match (warmed, came) {
        (Err(why), _) => println!("local: the script prompt was not cached: {why}"),
        (Ok(()), None) if lang == Lang::C => println!("local: the C card cached in {ms} ms"),
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
        "script" | "program" | "app" => script(backends, text, response),
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
        "query" => query(backends, text, response),
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

/// A quarantined read: the model's answer to one typed question about the
/// data, checked against its kind here, so the shell gets a word or one short
/// line, or why there is none. It never touches the conversation, but it
/// takes the script slot: the next script request prefills its prompt again.
fn query(backends: &mut Backends, text: &str, response: &mut [u8]) -> Result<String, String> {
    let request = QueryRequest::decode(text).ok_or("malformed query request")?;
    let completion = match (&mut backends.local, &backends.server) {
        (Some(Ok(local)), _) => local.query(&request),
        (_, Some(server)) => server.query(&request, response),
        (Some(Err(why)), None) => Err(format!("the model cannot run: {why}")),
        (None, None) => Err(String::from(NO_MODEL)),
    };
    let answer = completion.and_then(|c| query::answer(&request.kind, &c));
    Ok(query::encode_answer(&answer))
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
