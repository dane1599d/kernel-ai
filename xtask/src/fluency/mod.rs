//! `cargo xtask fluency --llm HOST:PORT --model NAME [--repeat N]`, or
//! `cargo xtask fluency --local ID [--store DIR]`
//!
//! The M8b gate: does the model write scripts the shell accepts, and do they do
//! what was asked? Each task goes through the OS's own path: the same chat
//! request, answer parser, strict checks, one repair round, tier and sandboxed
//! run. With `--llm` the model is on an LLM server and only the
//! transport (a host TCP socket) is a stand-in; with `--local` it is a catalog
//! model run by Kernel AI's own engine on this PC exactly as the llm service runs
//! it ([`local`]). The OS is a [`GateHost`] with fixed values. The gate passes when
//! [`FIRST_TRY_GATE`]% of runs pass on the model's first answer and
//! [`REPAIRED_GATE`]% pass after at most one repair. Every run is written to
//! `target/fluency/<unix time>.jsonl`.

#[cfg(test)]
mod grain;
pub(crate) mod host;
mod judge;
pub(crate) mod local;
pub(crate) mod tasks;
#[cfg(test)]
mod tests;

use host::{GateHost, Transport};
use judge::{judge, no_proposal};
use llmproto::script::{self, AnswerError, ScriptRequest};
use llmproto::{http, openai};
use serde_json::{Value, json};
use shellcore::{DEFAULT_CONFIG, Shell};
use std::fmt::Write as _;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tasks::{TASKS, Task};

/// Percent of runs that must pass on the model's first answer.
const FIRST_TRY_GATE: usize = 70;
/// Percent of runs that must pass after at most one repair.
const REPAIRED_GATE: usize = 90;
const MAX_REPEAT: u32 = 10;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest detail printed for a failed run.
const DETAIL_CHARS: usize = 200;

/// Why a run failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The server or the local engine failed, or the answer was cut off.
    Server,
    /// The checks refused the script, and again after the repair.
    Refused,
    /// The model wrote no script for a request it could do.
    NoScript,
    /// The script stopped with an error.
    RunError,
    /// The output lacks something the task asks for.
    WrongOutput,
    /// The policy changes are not the ones asked for.
    WrongChange,
    /// An impossible request was not refused.
    NotRefused,
}

impl Failure {
    const ALL: [Self; 7] = [
        Self::Server,
        Self::Refused,
        Self::NoScript,
        Self::RunError,
        Self::WrongOutput,
        Self::WrongChange,
        Self::NotRefused,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Refused => "refused",
            Self::NoScript => "no-script",
            Self::RunError => "run-error",
            Self::WrongOutput => "wrong-output",
            Self::WrongChange => "wrong-change",
            Self::NotRefused => "not-refused",
        }
    }
}

/// One run of one task.
#[derive(Debug)]
pub struct Outcome {
    /// Script requests made: 1, or 2 with a repair.
    pub rounds: usize,
    /// The script that ran, if one was proposed.
    pub script: Option<String>,
    /// What the shell showed for the run, or why nothing ran.
    pub output: String,
    pub failure: Option<(Failure, String)>,
}

impl Outcome {
    fn first_try(&self) -> bool {
        self.failure.is_none() && self.rounds == 1
    }
}

/// Runs `task` through the shell the way a user would: `? <request>`, then `yes`
/// to a script that asks; a run-tier one runs at once.
pub fn attempt(task: &Task, transport: Transport) -> Outcome {
    attempt_on(task, GateHost::new(transport))
}

/// [`attempt`] on `host`, which may answer `act` requests too.
pub fn attempt_on(task: &Task, host: GateHost) -> Outcome {
    let host = Rc::new(host);
    let mut shell = Shell::new(host.clone(), DEFAULT_CONFIG);
    let proposal = shell.eval(&format!("? {}", task.request));
    let answers = host.answers.borrow().clone();
    let rounds = answers.len();
    let script = answers
        .iter()
        .rev()
        .find_map(|answer| answer.as_ref().ok())
        .map(|answer| answer.script.clone());
    if let Some(output) = shell.last_output() {
        let failure = judge(task, proposal.ok, output, &host.changes.borrow());
        return Outcome {
            rounds,
            script,
            output: output.to_string(),
            failure,
        };
    }
    let Some(id) = proposal.confirm else {
        // Through the one door, words come back as `say` (or `clarify`), not as
        // an empty script: the same answer in words as today's `NoScript`.
        let worded = host.calls.borrow().last().and_then(|call| match call {
            Ok(call) if answers.is_empty() && matches!(call.name.as_str(), "say" | "clarify") => {
                Some(Err(AnswerError::NoScript(proposal.text.clone())))
            }
            _ => None,
        });
        let failure = no_proposal(task, worded.as_ref().or(answers.last()), &proposal.text);
        return Outcome {
            rounds,
            script: None,
            output: proposal.text,
            failure,
        };
    };
    let reply = shell.eval(&format!("answer {id} yes"));
    // The script's own output, without the shell's line saying what changed.
    let output = shell.last_output().unwrap_or(&reply.text).to_string();
    let failure = judge(task, reply.ok, &output, &host.changes.borrow());
    Outcome {
        rounds,
        script,
        output,
        failure,
    }
}

/// Sends script requests to the LLM server, as the llm service does.
fn server_transport(server: String, model: String) -> Transport {
    Box::new(move |request: &ScriptRequest| {
        let (body, timeout) = service_request(&model, request)?;
        let response = exchange(&server, &body, timeout).map_err(AnswerError::Server)?;
        script_answer(&response)
    })
}

/// A server's response read as the llm service reads it.
fn script_answer(response: &http::Response) -> Result<script::Answer, AnswerError> {
    openai::completion(response)
        .map_err(script::server_error)
        .and_then(|completion| script::answer(&completion))
}

/// The chat request body and time limit the llm service would use for `request`.
/// It goes through the wire format first, as the shell sends it, so it is clipped
/// exactly as in the OS.
fn service_request(
    model: &str,
    request: &ScriptRequest,
) -> Result<(Vec<u8>, Duration), AnswerError> {
    let request = ScriptRequest::decode(&request.encode())
        .ok_or_else(|| AnswerError::Server("the request does not fit the wire format".into()))?;
    let repair = request
        .repair
        .as_ref()
        .map(|(script, error)| script::Repair { script, error });
    let body = script::chat_request(model, &request.request, repair);
    let timeout = Duration::from_millis(request.timeout_ms.min(script::SERVICE_TIMEOUT_MS));
    Ok((body, timeout))
}

/// Connects to the first address of `server` that answers ("localhost" may name
/// ::1 first, while the server listens on 127.0.0.1 only).
fn connect(server: &str) -> Result<TcpStream, String> {
    let mut why = format!("{server}: no address");
    for address in server
        .to_socket_addrs()
        .map_err(|error| format!("{server}: {error}"))?
    {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => why = format!("connect to {server} ({address}): {error}"),
        }
    }
    Err(why)
}

/// One HTTP exchange with the server, finished within `timeout`.
fn exchange(server: &str, body: &[u8], timeout: Duration) -> Result<http::Response, String> {
    let deadline = Instant::now() + timeout;
    let mut stream = connect(server)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    stream
        .write_all(&http::post_json(server, openai::CHAT_PATH, None, body))
        .map_err(|error| format!("send: {error}"))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!("no answer within {} s", timeout.as_secs()));
        }
        stream
            .set_read_timeout(Some(left))
            .map_err(|error| error.to_string())?;
        let read = match stream.read(&mut chunk) {
            Ok(read) => read,
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                continue;
            }
            Err(error) => return Err(format!("receive: {error}")),
        };
        buf.extend_from_slice(&chunk[..read]);
        match http::parse_response(&buf, read == 0) {
            Err(http::ParseError::Incomplete) if read > 0 => {}
            result => return result.map_err(|error| format!("bad response: {error:?}")),
        }
    }
}

/// What answers the script requests.
#[derive(Debug, PartialEq, Eq)]
enum Backend {
    Server {
        server: String,
        model: String,
    },
    /// A catalog model in the store, run by Kernel AI's own engine.
    Local {
        id: String,
        store: Option<String>,
    },
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    backend: Backend,
    repeat: u32,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let (mut server, mut model, mut local, mut store, mut repeat) = (None, None, None, None, 1);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = |what: &str| rest.next().cloned().ok_or(format!("{arg} expects {what}"));
        match arg.as_str() {
            "--llm" => server = Some(value("HOST:PORT")?),
            "--model" => model = Some(value("a model name")?),
            "--local" => local = Some(value("a catalog model id")?),
            "--store" => store = Some(value("a directory")?),
            "--repeat" => {
                repeat = value("a count")?
                    .parse()
                    .ok()
                    .filter(|n| (1..=MAX_REPEAT).contains(n))
                    .ok_or(format!("--repeat expects 1 to {MAX_REPEAT}"))?;
            }
            other => return Err(format!("unknown fluency option {other}")),
        }
    }
    let backend = match (local, server, model) {
        // Seeded draws on our engine give the same answers every run.
        (Some(_), None, None) if repeat > 1 => {
            return Err(
                "--repeat goes with --llm: a --local run gives the same answers every time".into(),
            );
        }
        (Some(id), None, None) => Backend::Local { id, store },
        (Some(_), _, _) => return Err("--local takes no --llm or --model".into()),
        (None, Some(_), Some(_)) if store.is_some() => {
            return Err("--store goes with --local".into());
        }
        (None, server, model) => Backend::Server {
            server: server.ok_or("fluency needs --llm HOST:PORT, or --local ID")?,
            model: model.ok_or("fluency needs --model NAME")?,
        },
    };
    Ok(Options { backend, repeat })
}

/// Pass counts over a set of runs.
#[derive(Debug, PartialEq, Eq)]
struct Summary {
    runs: usize,
    first_try: usize,
    passed: usize,
}

impl Summary {
    fn of(outcomes: &[Outcome]) -> Self {
        Self {
            runs: outcomes.len(),
            first_try: outcomes.iter().filter(|o| o.first_try()).count(),
            passed: outcomes.iter().filter(|o| o.failure.is_none()).count(),
        }
    }

    fn meets_gate(&self) -> bool {
        self.runs > 0
            && self.first_try * 100 >= FIRST_TRY_GATE * self.runs
            && self.passed * 100 >= REPAIRED_GATE * self.runs
    }
}

fn percent(part: usize, whole: usize) -> usize {
    part * 100 / whole.max(1)
}

fn clip(text: &str, max: usize) -> String {
    let flat = text.replace('\n', " | ");
    match flat.char_indices().nth(max) {
        Some((cut, _)) => format!("{}...", &flat[..cut]),
        None => flat,
    }
}

fn record(number: usize, repeat: u32, task: &Task, outcome: &Outcome, ms: u128) -> Value {
    json!({
        "task": number + 1,
        "repeat": repeat + 1,
        "request": task.request,
        "reference": task.reference,
        "rounds": outcome.rounds,
        "passed": outcome.failure.is_none(),
        "first_try": outcome.first_try(),
        "failure": outcome.failure.as_ref().map(|(kind, _)| kind.name()),
        "detail": outcome.failure.as_ref().map(|(_, detail)| detail),
        "script": outcome.script,
        "output": outcome.output,
        "ms": ms,
    })
}

fn show(number: usize, task: &Task, outcome: &Outcome, ms: u128) {
    let rounds = if outcome.rounds == 2 {
        ", repaired"
    } else {
        ""
    };
    match &outcome.failure {
        None => println!(
            "  ok   {:>2} {} ({ms} ms{rounds})",
            number + 1,
            task.request
        ),
        Some((kind, detail)) => {
            println!(
                "  FAIL {:>2} {} ({ms} ms{rounds})",
                number + 1,
                task.request
            );
            println!("         {}: {}", kind.name(), clip(detail, DETAIL_CHARS));
            if let Some(script) = &outcome.script {
                println!("         script: {}", clip(script, DETAIL_CHARS));
            }
        }
    }
}

/// The task file with LF line ends, as hashed into the report.
fn tasks_text() -> String {
    include_str!("tasks.rs").replace("\r\n", "\n")
}

/// `cargo xtask fluency`: runs every task, writes the runs, and fails below the gate.
pub fn main(args: &[String], target_dir: &Path) -> Result<(), String> {
    let options = parse(args)?;
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    // A transport for each attempt, and how the header names the backend.
    let (transport, model, server): (Box<dyn Fn() -> Transport>, String, String) =
        match &options.backend {
            Backend::Server { server, model } => {
                let (s, m) = (server.clone(), model.clone());
                (
                    Box::new(move || server_transport(s.clone(), m.clone())),
                    model.clone(),
                    server.clone(),
                )
            }
            Backend::Local { id, store } => {
                let (engine, line) = local::load(id, store.as_deref())?;
                println!("xtask: fluency: local model {line}");
                (
                    Box::new(move || local::transport(engine.clone())),
                    id.clone(),
                    format!("local: {line}"),
                )
            }
        };
    let mut records = vec![json!({
        "model": model,
        "server": server,
        "repeat": options.repeat,
        "tasks": TASKS.len(),
        // Normalised, so CRLF and LF checkouts of one commit record the same hash.
        "tasks_sha256": crate::sha256_hex(tasks_text().as_bytes()),
        "started_unix": started,
    })];
    println!(
        "xtask: fluency gate, {} tasks x {} against {server} ({model})",
        TASKS.len(),
        options.repeat,
    );
    let mut outcomes = Vec::new();
    for repeat in 0..options.repeat {
        for (number, task) in TASKS.iter().enumerate() {
            let clock = Instant::now();
            let outcome = attempt(task, transport());
            let ms = clock.elapsed().as_millis();
            show(number, task, &outcome, ms);
            records.push(record(number, repeat, task, &outcome, ms));
            outcomes.push(outcome);
        }
    }
    let summary = Summary::of(&outcomes);
    let mut failures = String::new();
    for kind in Failure::ALL {
        let count = outcomes
            .iter()
            .filter(|o| o.failure.as_ref().is_some_and(|(k, _)| *k == kind))
            .count();
        if count > 0 {
            let _ = write!(failures, " {} {count}", kind.name());
        }
    }
    records.push(json!({
        "runs": summary.runs,
        "first_try": summary.first_try,
        "passed": summary.passed,
        "gate_first_try_percent": FIRST_TRY_GATE,
        "gate_repaired_percent": REPAIRED_GATE,
        "meets_gate": summary.meets_gate(),
    }));
    let dir = target_dir.join("fluency");
    fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let path = dir.join(format!("{started}.jsonl"));
    let lines: Vec<String> = records.iter().map(Value::to_string).collect();
    fs::write(&path, lines.join("\n") + "\n")
        .map_err(|error| format!("{}: {error}", path.display()))?;
    println!(
        "xtask: fluency: {}/{} first try ({}%), {}/{} after one repair ({}%); gate {FIRST_TRY_GATE}% and {REPAIRED_GATE}%",
        summary.first_try,
        summary.runs,
        percent(summary.first_try, summary.runs),
        summary.passed,
        summary.runs,
        percent(summary.passed, summary.runs),
    );
    if !failures.is_empty() {
        println!("xtask: failures:{failures}");
    }
    println!("xtask: runs written to {}", path.display());
    if summary.meets_gate() {
        println!("xtask: the fluency gate passes");
        Ok(())
    } else {
        Err("the fluency gate fails".into())
    }
}
