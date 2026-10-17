//! `cargo xtask fluency --llm HOST:PORT --model NAME [--repeat N]`
//!
//! The M8b gate: does the model on the LLM server write scripts the shell accepts,
//! and do they do what was asked? Each task goes through the OS's own path: the same
//! chat request, answer parser, strict checks, one repair round, confirmation and
//! sandboxed run. Only the transport (a host TCP socket) and the OS (a [`GateHost`]
//! with fixed values) are stand-ins. The gate passes when [`FIRST_TRY_GATE`]% of runs
//! pass on the model's first answer and [`REPAIRED_GATE`]% pass after at most one
//! repair. Every run is written to `target/fluency/<unix time>.jsonl`.

mod host;
mod tasks;
#[cfg(test)]
mod tests;

use host::{GateHost, Transport};
use llmproto::script::{self, Answer, AnswerError, ScriptRequest};
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
use tasks::{Policy, REFUSAL_WORDS, TASKS, Task};

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
    /// The server failed or the answer was cut off.
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

/// Runs `task` through the shell the way a user would: `? <request>`, then `yes`.
pub fn attempt(task: &Task, transport: Transport) -> Outcome {
    let host = Rc::new(GateHost::new(transport));
    let mut shell = Shell::new(host.clone(), DEFAULT_CONFIG);
    let proposal = shell.eval(&format!("? {}", task.request));
    let answers = host.answers.borrow().clone();
    let rounds = answers.len();
    let Some(id) = proposal.confirm else {
        let failure = no_proposal(task, answers.last(), &proposal.text);
        return Outcome {
            rounds,
            script: None,
            output: proposal.text,
            failure,
        };
    };
    let script = answers
        .iter()
        .rev()
        .find_map(|answer| answer.as_ref().ok())
        .map(|answer| answer.script.clone());
    let reply = shell.eval(&format!("answer {id} yes"));
    let failure = judge(task, reply.ok, &reply.text, &host.changes.borrow());
    Outcome {
        rounds,
        script,
        output: reply.text,
        failure,
    }
}

/// No script was proposed. That fails, except for an impossible request the model
/// declined to write a script for.
fn no_proposal(
    task: &Task,
    last: Option<&Result<Answer, AnswerError>>,
    text: &str,
) -> Option<(Failure, String)> {
    let failure = match last {
        Some(Err(AnswerError::NoScript(_))) if task.refusal => return None,
        Some(Err(AnswerError::NoScript(_))) => Failure::NoScript,
        Some(Err(AnswerError::Server(_) | AnswerError::CutOff)) | None => Failure::Server,
        Some(_) => Failure::Refused,
    };
    Some((failure, text.to_string()))
}

/// Whether a run that printed `output` and made `changes` did what `task` asks.
fn judge(task: &Task, ok: bool, output: &str, changes: &[u64]) -> Option<(Failure, String)> {
    if !ok {
        return Some((Failure::RunError, output.to_string()));
    }
    if task.refusal {
        if !changes.is_empty() {
            return Some((Failure::WrongChange, format!("policy changes {changes:?}")));
        }
        if !says_refusal(output) {
            return Some((Failure::NotRefused, output.to_string()));
        }
        return None;
    }
    if let Some(missing) = task
        .prints
        .iter()
        .find(|piece| !contains_piece(output, piece))
    {
        return Some((
            Failure::WrongOutput,
            format!("no {missing:?} in {output:?}"),
        ));
    }
    let all_to = |name| changes.iter().all(|&id| id == policy_id(name));
    let right = match task.policy {
        Policy::Unchanged => changes.is_empty(),
        Policy::Set(name) => !changes.is_empty() && all_to(name),
        Policy::SetOrKeep(name) => all_to(name),
    };
    (!right).then(|| {
        (
            Failure::WrongChange,
            format!("policy changes {changes:?}, wanted {:?}", task.policy),
        )
    })
}

/// Whether `output` says the request cannot be done: it holds one of
/// [`REFUSAL_WORDS`] as whole words, with at most one other word inside the phrase.
fn says_refusal(output: &str) -> bool {
    let words = words_of(output);
    REFUSAL_WORDS.iter().any(|phrase| {
        let phrase = words_of(phrase);
        (0..words.len()).any(|start| matches_from(&words[start..], &phrase, 1))
    })
}

/// Whether `words` starts with `phrase`, skipping at most `gaps` extra words
/// after its first word.
fn matches_from(words: &[String], phrase: &[String], gaps: usize) -> bool {
    match (words.first(), phrase.first()) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(word), Some(first)) if word == first => {
            let rest = &phrase[1..];
            matches_from(&words[1..], rest, gaps)
                || (gaps > 0
                    && !rest.is_empty()
                    && words.len() > 1
                    && matches_from(&words[2..], rest, gaps - 1))
        }
        _ => false,
    }
}

/// Lower-case words, apostrophes kept so "can't" is one word.
fn words_of(text: &str) -> Vec<String> {
    text.to_ascii_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '\''))
        .filter(|word| !word.is_empty())
        .map(String::from)
        .collect()
}

fn policy_id(name: &str) -> u64 {
    if name == "round-robin" {
        kabi::policy::ROUND_ROBIN
    } else {
        kabi::policy::INTERACTIVE
    }
}

/// Whether `output` contains `piece`. A piece that starts or ends with a digit must
/// not run on into more of a number there: "4" is not in "4321" or "4.321", though
/// it is in "4 s" and in "it is 4.". A piece without capitals matches in any case.
fn contains_piece(output: &str, piece: &str) -> bool {
    let haystack = if piece.chars().any(|c| c.is_ascii_uppercase()) {
        output.to_string()
    } else {
        output.to_ascii_lowercase()
    };
    let bytes = haystack.as_bytes();
    let digit = |index: usize| bytes.get(index).is_some_and(u8::is_ascii_digit);
    let point = |index: usize| bytes.get(index) == Some(&b'.');
    // A digit, or a decimal point with a digit beyond it, carries a number on.
    let runs_on_after = |end: usize| digit(end) || (point(end) && digit(end + 1));
    let runs_on_before = |start: usize| {
        start > 0 && (digit(start - 1) || (start > 1 && point(start - 1) && digit(start - 2)))
    };
    let starts_digit = piece.starts_with(|c: char| c.is_ascii_digit());
    let ends_digit = piece.ends_with(|c: char| c.is_ascii_digit());
    haystack.match_indices(piece).any(|(start, _)| {
        !(starts_digit && runs_on_before(start))
            && !(ends_digit && runs_on_after(start + piece.len()))
    })
}

/// Sends script requests to the LLM server, as the llm service does.
fn server_transport(server: String, model: String) -> Transport {
    Box::new(move |request: &ScriptRequest| {
        let (body, timeout) = service_request(&model, request)?;
        let response = exchange(&server, &body, timeout).map_err(AnswerError::Server)?;
        script::answer_from(&response)
    })
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

#[derive(Debug, PartialEq, Eq)]
struct Options {
    server: String,
    model: String,
    repeat: u32,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let (mut server, mut model, mut repeat) = (None, None, 1);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--llm" => server = Some(rest.next().ok_or("--llm expects HOST:PORT")?.clone()),
            "--model" => model = Some(rest.next().ok_or("--model expects a model name")?.clone()),
            "--repeat" => {
                repeat = rest
                    .next()
                    .and_then(|n| n.parse().ok())
                    .filter(|n| (1..=MAX_REPEAT).contains(n))
                    .ok_or(format!("--repeat expects 1 to {MAX_REPEAT}"))?;
            }
            other => return Err(format!("unknown fluency option {other}")),
        }
    }
    Ok(Options {
        server: server.ok_or("fluency needs --llm HOST:PORT")?,
        model: model.ok_or("fluency needs --model NAME")?,
        repeat,
    })
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
    let mut records = vec![json!({
        "model": options.model,
        "server": options.server,
        "repeat": options.repeat,
        "tasks": TASKS.len(),
        // Normalised, so CRLF and LF checkouts of one commit record the same hash.
        "tasks_sha256": crate::sha256_hex(tasks_text().as_bytes()),
        "started_unix": started,
    })];
    println!(
        "xtask: fluency gate, {} tasks x {} against {} ({})",
        TASKS.len(),
        options.repeat,
        options.server,
        options.model
    );
    let mut outcomes = Vec::new();
    for repeat in 0..options.repeat {
        for (number, task) in TASKS.iter().enumerate() {
            let clock = Instant::now();
            let transport = server_transport(options.server.clone(), options.model.clone());
            let outcome = attempt(task, transport);
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
