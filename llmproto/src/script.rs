//! LLM-written Rhai scripts: the API a model may use, the prompt that teaches it, the
//! chat request, the checks on its answer, and the framing between the shell and
//! the llm service. The OS and the host-side fluency harness both use exactly this
//! code, so the gate measures what ships.
//!
//! Nothing on this path is ever cut short: an answer over a limit, or with a
//! character the console could not show faithfully, is refused, because what the
//! user is asked about, or sees with `show`, must be exactly what runs.

use crate::chat::{self, Completion, Message, Role};
use crate::openai::{CompletionError, ReplyError};
use crate::text;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use serde::Deserialize;

pub const SCRIPT_MAX_LINES: usize = 40;
pub const SCRIPT_MAX_LINE_CHARS: usize = 120;
pub const SCRIPT_MAX_BYTES: usize = 3 * 1024;
/// Short enough that `model says: <summary>` fits one row of the console.
pub const SUMMARY_MAX_CHARS: usize = 100;
/// Longest reply when the model answers in words (an empty script): the summary
/// then holds the answer itself, which the terminal wraps.
pub const REPLY_MAX_CHARS: usize = 300;
pub const REQUEST_MAX_BYTES: usize = 512;
pub const ERROR_MAX_BYTES: usize = 512;
/// Longest the llm service spends on one script request, whatever the shell asks.
pub const SERVICE_TIMEOUT_MS: u64 = 90_000;
pub const SCRIPT_MAX_TOKENS: u32 = 1024;
/// Sampling: low temperature and a fixed seed, so gate runs can be repeated. The
/// local model (`crate::local`) draws with the same two.
pub const TEMPERATURE: f32 = 0.2;
pub const SEED: u64 = 42;

/// Whether calling a function only reads the system or changes it.
pub use agentabi::Effect;

/// One function LLM-written scripts may call: a row of the action registry.
pub type ApiFn = agentabi::Action;

/// Every function an LLM-written script may call, generated from the action
/// registry (`agentabi::SCRIPT_API`); each has one arity and one effect.
pub const API: &[ApiFn] = &agentabi::SCRIPT_API;

/// Worked examples shown to the model: (request, script, summary).
const EXAMPLES: &[(&str, &str, &str)] = &[
    (
        "how many CPUs are there?",
        "print(\"CPUs: \" + cpus());",
        "Prints the number of CPUs.",
    ),
    (
        "what percentage of memory is free?",
        "let m = memory();\nlet pct = m.free_mib * 100 / m.total_mib;\nprint(\"free: \" + pct + \"%\");",
        "Prints free memory as a whole percentage.",
    ),
    (
        "use round robin scheduling unless it is on already",
        "if get_policy() != \"round-robin\" {\n    set_policy(\"round-robin\");\n}\nprint(\"policy: \" + get_policy());",
        "Switches to round-robin if needed and prints the policy.",
    ),
    (
        "print the names of all services",
        "let names = \"\";\nfor p in processes() {\n    if names != \"\" {\n        names += \", \";\n    }\n    names += p.name;\n}\nprint(names);",
        "Prints every service name, separated by commas.",
    ),
    (
        "hello",
        "",
        "Hello! Ask me about this system, or tell me what to do.",
    ),
    (
        "what can you do?",
        "",
        "I answer questions and run scripts that read CPUs, memory, uptime and services, or set the policy. Try: how much memory is free? or use round robin.",
    ),
    (
        "delete all my files",
        "print(\"I cannot do that: scripts here have no access to files.\");",
        "Prints why it cannot be done.",
    ),
];

/// The system prompt. It is the same for every request, so a server can reuse it.
pub fn system_prompt() -> String {
    let mut prompt = String::from(
        "You write short Rhai scripts for the shell of Kernel AI, a small operating system.\n\
         Answer with JSON only: {\"script\": \"...\", \"summary\": \"...\"}.\n\
         script: Rhai code, at most 40 lines of at most 120 characters, ASCII only.\n\
         summary: for a script, one sentence of at most 100 characters saying what it does.\n\
         No function needed (greeting, chat, a question you can answer): an empty script, and the summary is your answer to the user, at most 300 characters, never a description of it.\n\
         \n\
         Rhai rules here:\n\
         - End statements with ';'. Declare variables with let.\n\
         - Integers only: never write 1.5 or 100.0. Percent: x * 100 / y.\n\
         - / rounds down (7 / 2 is 3), so rounded down needs nothing more.\n\
         - Decimals: fixed(n, places) is n / 10^places as text with that many decimals: fixed(4321, 3) is \"4.321\".\n\
         - uptime_ms() is milliseconds: seconds are uptime_ms() / 1000; of s seconds, s / 60 are minutes, s % 60 seconds.\n\
         - Digits or characters of a number n: n.to_string().len(), never a loop.\n\
         - Loops: only for x in list { } or for i in 0..n { }. No while, loop or do.\n\
         - The first two items of a list are p[0] and p[1]: no loop needed.\n\
         - No closures, no fn definitions, no eval.\n\
         - Join strings with +: \"a\" + b. print(value) prints one line.\n\
         - if needs braces: if a > b { print(\"yes\"); } else { print(\"no\"); }\n\
         - Arrays: [1, 2], a.len(), a[0], a[a.len() - 1]. Maps: #{a: 1, b: 2}, one key per value asked for; m.a, m[\"a\"].\n\
         - A field is a name after a dot: memory().free_mib, processes()[0].name.\n\
         - Call only the functions below, fixed, and Rhai's built-in string, array and map methods.\n\
         - Print exactly what is asked, in its format, with its words and units (MiB, KiB).\n\
         - Do the steps in the order asked: print first if the request prints first.\n\
         - If the request cannot be done with these functions, print why instead.\n\
         \n\
         Functions:\n",
    );
    for function in API {
        let _ = writeln!(prompt, "- {}: {}", function.signature(), function.doc);
    }
    prompt.push_str("\nExamples:\n");
    for (request, script, summary) in EXAMPLES {
        let envelope = serde_json::json!({ "script": script, "summary": summary });
        let _ = writeln!(prompt, "Request: {request}\n{envelope}");
    }
    prompt
}

/// The `response_format` asking for the envelope. Script first, so the summary is
/// written knowing the code.
pub fn response_format() -> serde_json::Value {
    serde_json::json!({
        "type": "json_schema",
        "json_schema": {
            "name": "rhai_script",
            "strict": true,
            "schema": {
                "type": "object",
                "properties": {
                    "script": { "type": "string" },
                    "summary": { "type": "string" }
                },
                "required": ["script", "summary"],
                "additionalProperties": false
            }
        }
    })
}

/// A previous answer and why it was refused, for a repair round.
#[derive(Clone, Copy, Debug)]
pub struct Repair<'a> {
    pub script: &'a str,
    pub error: &'a str,
}

/// The conversation for `request`: the system prompt and the request, then, for a
/// repair, the refused answer and why it was refused.
pub fn request(request: &str, repair: Option<Repair<'_>>) -> chat::Request {
    let user = format!("Request: {request}\n/no_think");
    let mut asked = chat::Request::new(Some(&system_prompt()), &user, SCRIPT_MAX_TOKENS);
    if let Some(repair) = repair {
        let previous = serde_json::json!({ "script": repair.script, "summary": "" }).to_string();
        let feedback = format!(
            "That script was refused: {}\nWrite a corrected script.\n/no_think",
            repair.error
        );
        asked
            .messages
            .push(Message::new(Role::Assistant, &previous));
        asked.messages.push(Message::new(Role::User, &feedback));
    }
    asked
}

/// The chat request body for `request` (and, for a repair, the refused answer).
pub fn chat_request(model: &str, request: &str, repair: Option<Repair<'_>>) -> Vec<u8> {
    let asked = self::request(request, repair);
    let body = serde_json::json!({
        "model": model,
        "messages": asked.messages,
        "max_tokens": asked.max_tokens,
        "temperature": TEMPERATURE,
        "seed": SEED,
        "stream": false,
        "response_format": response_format(),
        "chat_template_kwargs": { "enable_thinking": false },
    });
    serde_json::to_vec(&body).unwrap_or_default()
}

/// A checked answer: a one-line summary and a script within every limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub summary: String,
    pub script: String,
}

/// Why an answer was refused. [`AnswerError::repairable`] says which ones are worth
/// sending back to the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnswerError {
    /// The server failed or the reply was not a chat completion.
    Server(String),
    /// The model stopped at the token limit.
    CutOff,
    /// The content was not the JSON envelope.
    NoEnvelope,
    /// The script or summary broke a limit or used a forbidden character.
    Rejected(String),
    /// The model wrote no script; the summary may say why.
    NoScript(String),
    /// No backend can write scripts: neither a local model nor an LLM server. The
    /// text says what to do, in plain words.
    Unavailable(String),
    /// The local model failed or ran out of time.
    Local(String),
}

impl AnswerError {
    pub fn repairable(&self) -> bool {
        matches!(self, Self::NoEnvelope | Self::Rejected(_))
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Server(why) => format!("LLM server: {why}"),
            Self::Local(why) => format!("local model: {why}"),
            Self::CutOff => "the answer was cut off at the token limit".into(),
            Self::NoEnvelope => "the answer was not the JSON object asked for".into(),
            Self::Rejected(why) => why.clone(),
            Self::NoScript(summary) => format!("the model wrote no script: {summary}"),
            Self::Unavailable(why) => why.clone(),
        }
    }
}

#[derive(Deserialize)]
struct Envelope {
    script: String,
    #[serde(default)]
    summary: String,
}

/// Checks a script's text: line endings and tabs are normalised, then every byte
/// must be printable ASCII or a newline, and the size limits must hold. Returns the
/// normalised text, which is what is shown, checked and run.
pub fn check_text(script: &str) -> Result<String, AnswerError> {
    let normal = script.replace("\r\n", "\n").replace('\t', "    ");
    let normal = normal.trim_end().to_string();
    if let Some(bad) = normal
        .chars()
        .find(|&c| c != '\n' && !(' '..='~').contains(&c))
    {
        return Err(AnswerError::Rejected(format!(
            "the script has the character U+{:04X}; use ASCII only (escape others in strings)",
            u32::from(bad)
        )));
    }
    if normal.len() > SCRIPT_MAX_BYTES {
        return Err(AnswerError::Rejected(format!(
            "the script is {} bytes; the limit is {SCRIPT_MAX_BYTES}",
            normal.len()
        )));
    }
    let lines = normal.lines().count();
    if lines > SCRIPT_MAX_LINES {
        return Err(AnswerError::Rejected(format!(
            "the script has {lines} lines; the limit is {SCRIPT_MAX_LINES}"
        )));
    }
    if let Some((number, _)) = normal
        .lines()
        .enumerate()
        .find(|(_, line)| line.len() > SCRIPT_MAX_LINE_CHARS)
    {
        return Err(AnswerError::Rejected(format!(
            "line {} is longer than {SCRIPT_MAX_LINE_CHARS} characters",
            number + 1
        )));
    }
    Ok(normal)
}

/// Checks a script's summary: one line, printable, within the limit.
pub fn check_summary(summary: &str) -> Result<String, AnswerError> {
    check_line(summary, SUMMARY_MAX_CHARS)
}

/// Checks a reply in words: one line, printable, within its longer limit.
pub fn check_reply(reply: &str) -> Result<String, AnswerError> {
    check_line(reply, REPLY_MAX_CHARS)
}

fn check_line(text: &str, max_chars: usize) -> Result<String, AnswerError> {
    let text = text.trim();
    if text.chars().any(char::is_control) {
        return Err(AnswerError::Rejected(
            "the summary must be one line of plain text".into(),
        ));
    }
    if text.chars().count() > max_chars {
        return Err(AnswerError::Rejected(format!(
            "the summary is longer than {max_chars} characters"
        )));
    }
    Ok(text.to_string())
}

/// Reads a completion as a script answer.
pub fn answer(completion: &Completion) -> Result<Answer, AnswerError> {
    if completion.cut_off {
        return Err(AnswerError::CutOff);
    }
    if completion.text.is_empty() {
        return Err(AnswerError::Server(String::from(
            "the reply had no content",
        )));
    }
    let mut content = text::without_thinking(&completion.text);
    if let Some(fenced) = content
        .strip_prefix("```json")
        .or_else(|| content.strip_prefix("```"))
    {
        content = fenced.trim().strip_suffix("```").unwrap_or(fenced).trim();
    }
    let envelope: Envelope = serde_json::from_str(content).map_err(|_| AnswerError::NoEnvelope)?;
    let script = check_text(&envelope.script)?;
    if script.trim().is_empty() {
        return Err(AnswerError::NoScript(check_reply(&envelope.summary)?));
    }
    let summary = check_summary(&envelope.summary)?;
    Ok(Answer { summary, script })
}

/// Why a server's response held no script answer.
pub fn server_error(error: CompletionError) -> AnswerError {
    match error {
        CompletionError::Status(status, body) => AnswerError::Server(format!(
            "HTTP {status}: {}",
            text::printable(&body, SUMMARY_MAX_CHARS, 1)
        )),
        CompletionError::Reply(ReplyError::Server(why)) => AnswerError::Server(why),
        CompletionError::Reply(other) => AnswerError::Server(format!("{other:?}")),
    }
}

/// What the shell asks the llm service for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptRequest {
    pub request: String,
    /// The refused script and why, for a repair round.
    pub repair: Option<(String, String)>,
    /// How long the service may spend on this request.
    pub timeout_ms: u64,
}

/// Cuts `text` to at most `max` bytes at a character boundary (for error text only).
fn clip(text: &str, max: usize) -> &str {
    &text[..text.floor_char_boundary(max)]
}

impl ScriptRequest {
    /// `script <timeout> <request len> <script len> <error len>`, a newline, then
    /// the three texts back to back. Lengths make the framing exact whatever the
    /// texts contain.
    pub fn encode(&self) -> String {
        let request = clip(&self.request, REQUEST_MAX_BYTES);
        let (script, error) = self.repair.as_ref().map_or(("", ""), |(script, error)| {
            (script.as_str(), clip(error, ERROR_MAX_BYTES))
        });
        format!(
            "script {} {} {} {}\n{request}{script}{error}",
            self.timeout_ms,
            request.len(),
            script.len(),
            error.len()
        )
    }

    pub fn decode(text: &str) -> Option<Self> {
        let (header, body) = text.split_once('\n')?;
        let mut words = header.strip_prefix("script ")?.split(' ');
        let mut number = || words.next()?.parse::<usize>().ok();
        let (timeout, request_len, script_len, error_len) =
            (number()?, number()?, number()?, number()?);
        let total = request_len
            .checked_add(script_len)?
            .checked_add(error_len)?;
        if body.len() != total || request_len > REQUEST_MAX_BYTES || script_len > SCRIPT_MAX_BYTES {
            return None;
        }
        let request = body.get(..request_len)?;
        let script = body.get(request_len..request_len + script_len)?;
        let error = body.get(request_len + script_len..)?;
        Some(Self {
            request: request.to_string(),
            repair: (script_len > 0).then(|| (script.to_string(), error.to_string())),
            timeout_ms: timeout as u64,
        })
    }
}

/// `<summary len> <script len>`, a newline, then the summary and the script.
pub fn encode_answer(answer: &Answer) -> String {
    format!(
        "{} {}\n{}{}",
        answer.summary.len(),
        answer.script.len(),
        answer.summary,
        answer.script
    )
}

pub fn decode_answer(text: &str) -> Option<Answer> {
    let (header, body) = text.split_once('\n')?;
    let (summary_len, script_len) = header.split_once(' ')?;
    let summary_len: usize = summary_len.parse().ok()?;
    let script_len: usize = script_len.parse().ok()?;
    if body.len() != summary_len.checked_add(script_len)? {
        return None;
    }
    Some(Answer {
        summary: body.get(..summary_len)?.to_string(),
        script: body.get(summary_len..)?.to_string(),
    })
}

/// `<kind> <text>`, so the shell can tell which errors are worth a repair round.
pub fn encode_error(error: &AnswerError) -> String {
    match error {
        AnswerError::Server(why) => format!("server {why}"),
        AnswerError::CutOff => "cutoff".into(),
        AnswerError::NoEnvelope => "noenvelope".into(),
        AnswerError::Rejected(why) => format!("rejected {why}"),
        AnswerError::NoScript(summary) => format!("noscript {summary}"),
        AnswerError::Unavailable(why) => format!("unavailable {why}"),
        AnswerError::Local(why) => format!("local {why}"),
    }
}

/// Reads [`encode_error`]'s text. Anything else is a server error.
pub fn decode_error(text: &str) -> AnswerError {
    let (kind, rest) = text.split_once(' ').unwrap_or((text, ""));
    match kind {
        "server" => AnswerError::Server(rest.into()),
        "cutoff" => AnswerError::CutOff,
        "noenvelope" => AnswerError::NoEnvelope,
        "rejected" => AnswerError::Rejected(rest.into()),
        "noscript" => AnswerError::NoScript(rest.into()),
        "unavailable" => AnswerError::Unavailable(rest.into()),
        "local" => AnswerError::Local(rest.into()),
        _ => AnswerError::Server(text.into()),
    }
}

#[cfg(test)]
mod tests;
