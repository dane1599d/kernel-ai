//! LLM-written Rhai scripts: the API a model may use, the prompt that teaches it, the
//! chat request, the checks on its answer, and the framing between the shell and
//! the llm service. The OS and the host-side fluency harness both use exactly this
//! code, so the gate measures what ships.
//!
//! Nothing on this path is ever cut short: an answer over a limit, or with a
//! character the console could not show faithfully, is refused, because what the
//! user confirms must be exactly what runs.

use crate::openai::{self, ReplyError};
use crate::{http, text};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use serde::Deserialize;

pub const SCRIPT_MAX_LINES: usize = 40;
pub const SCRIPT_MAX_LINE_CHARS: usize = 120;
pub const SCRIPT_MAX_BYTES: usize = 3 * 1024;
pub const SUMMARY_MAX_CHARS: usize = 160;
pub const REQUEST_MAX_BYTES: usize = 512;
pub const ERROR_MAX_BYTES: usize = 512;
pub const SCRIPT_MAX_TOKENS: u32 = 1024;
/// Sampling: low temperature and a fixed seed, so gate runs can be repeated.
pub const TEMPERATURE: f32 = 0.2;
pub const SEED: u64 = 42;

/// Whether calling a function only reads the system or changes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Read,
    Change,
}

/// One function LLM-written scripts may call.
#[derive(Clone, Copy, Debug)]
pub struct ApiFn {
    pub name: &'static str,
    pub signature: &'static str,
    pub effect: Effect,
    pub doc: &'static str,
}

/// Every function an LLM-written script may call; each has one arity and one effect.
pub const API: &[ApiFn] = &[
    ApiFn {
        name: "cpus",
        signature: "cpus() -> int",
        effect: Effect::Read,
        doc: "number of CPUs online",
    },
    ApiFn {
        name: "memory",
        signature: "memory() -> #{free_mib: int, total_mib: int, heap_kib: int}",
        effect: Effect::Read,
        doc: "free and total memory in MiB, and the kernel heap in use in KiB",
    },
    ApiFn {
        name: "uptime_ms",
        signature: "uptime_ms() -> int",
        effect: Effect::Read,
        doc: "milliseconds since boot",
    },
    ApiFn {
        name: "processes",
        signature: "processes() -> [#{pid: int, name: string}]",
        effect: Effect::Read,
        doc: "the running services",
    },
    ApiFn {
        name: "threads",
        signature: "threads() -> #{switches: int, ejections: int, migrations: int}",
        effect: Effect::Read,
        doc: "scheduler counters since boot",
    },
    ApiFn {
        name: "get_policy",
        signature: "get_policy() -> string",
        effect: Effect::Read,
        doc: "the scheduling policy, \"interactive\" or \"round-robin\"",
    },
    ApiFn {
        name: "set_policy",
        signature: "set_policy(name: string)",
        effect: Effect::Change,
        doc: "switches every CPU to \"interactive\" or \"round-robin\"",
    },
];

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
        "delete all my files",
        "print(\"I cannot do that: scripts here have no access to files.\");",
        "Explains that there is no file access.",
    ),
];

/// The system prompt. It is the same for every request, so a server can reuse it.
pub fn system_prompt() -> String {
    let mut prompt = String::from(
        "You write short Rhai scripts for the shell of Kernel AI, a small operating system.\n\
         Answer with JSON only: {\"script\": \"...\", \"summary\": \"...\"}.\n\
         script: Rhai code, at most 40 lines of at most 120 characters, ASCII only.\n\
         summary: one sentence saying what the script does.\n\
         \n\
         Rhai rules here:\n\
         - End statements with ';'. Declare variables with let.\n\
         - Integers only, no floating point: for a percentage write x * 100 / y.\n\
         - No loops (for, while, loop, do), no closures, no fn definitions, no eval.\n\
         - Join strings with +: \"a\" + b. print(value) prints one line.\n\
         - if needs braces: if a > b { print(\"yes\"); } else { print(\"no\"); }\n\
         - Arrays: [1, 2], a.len(), a[0]. Maps: #{a: 1}, m.a, m[\"a\"].\n\
         - Call only the functions below and Rhai's built-in string, array and map methods.\n\
         - If the request cannot be done with these functions, print why instead.\n\
         \n\
         Functions:\n",
    );
    for function in API {
        let _ = writeln!(prompt, "- {}: {}", function.signature, function.doc);
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

/// The chat request for `request` (and, for a repair, the refused answer).
pub fn chat_request(model: &str, request: &str, repair: Option<Repair<'_>>) -> Vec<u8> {
    let system = system_prompt();
    let user = format!("Request: {request}\n/no_think");
    let mut messages = Vec::from([
        openai::ChatMessage {
            role: "system",
            content: &system,
        },
        openai::ChatMessage {
            role: "user",
            content: &user,
        },
    ]);
    let (previous, feedback);
    if let Some(repair) = repair {
        previous = serde_json::json!({ "script": repair.script, "summary": "" }).to_string();
        feedback = format!(
            "That script was refused: {}\nWrite a corrected script.\n/no_think",
            repair.error
        );
        messages.push(openai::ChatMessage {
            role: "assistant",
            content: &previous,
        });
        messages.push(openai::ChatMessage {
            role: "user",
            content: &feedback,
        });
    }
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "max_tokens": SCRIPT_MAX_TOKENS,
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
}

impl AnswerError {
    pub fn repairable(&self) -> bool {
        matches!(self, Self::NoEnvelope | Self::Rejected(_))
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Server(why) => format!("LLM server: {why}"),
            Self::CutOff => "the answer was cut off at the token limit".into(),
            Self::NoEnvelope => "the answer was not the JSON object asked for".into(),
            Self::Rejected(why) => why.clone(),
            Self::NoScript(summary) => format!("the model wrote no script: {summary}"),
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

/// Checks a summary: one line, printable, within the limit.
fn check_summary(summary: &str) -> Result<String, AnswerError> {
    let summary = summary.trim();
    if summary.chars().any(char::is_control) {
        return Err(AnswerError::Rejected(
            "the summary must be one line of plain text".into(),
        ));
    }
    if summary.chars().count() > SUMMARY_MAX_CHARS {
        return Err(AnswerError::Rejected(format!(
            "the summary is longer than {SUMMARY_MAX_CHARS} characters"
        )));
    }
    Ok(summary.to_string())
}

/// Reads a chat-completion response body as a script answer.
pub fn parse_answer(body: &[u8]) -> Result<Answer, AnswerError> {
    let reply = openai::parse_chat_reply(body).map_err(|error| match error {
        ReplyError::Server(why) => AnswerError::Server(why),
        other => AnswerError::Server(format!("{other:?}")),
    })?;
    if reply.finish_reason.as_deref() == Some("length") {
        return Err(AnswerError::CutOff);
    }
    let mut content = reply.content.trim();
    // A model that thinks anyway puts its reasoning first.
    if let Some(rest) = content.strip_prefix("<think>") {
        content = rest
            .split_once("</think>")
            .map_or("", |(_, after)| after)
            .trim();
    }
    if let Some(fenced) = content
        .strip_prefix("```json")
        .or_else(|| content.strip_prefix("```"))
    {
        content = fenced.trim().strip_suffix("```").unwrap_or(fenced).trim();
    }
    let envelope: Envelope = serde_json::from_str(content).map_err(|_| AnswerError::NoEnvelope)?;
    let summary = check_summary(&envelope.summary)?;
    let script = check_text(&envelope.script)?;
    if script.trim().is_empty() {
        return Err(AnswerError::NoScript(summary));
    }
    Ok(Answer { summary, script })
}

/// Reads a whole HTTP response as a script answer: the status first, then the body.
pub fn answer_from(response: &http::Response) -> Result<Answer, AnswerError> {
    if response.status != 200 {
        let body = core::str::from_utf8(&response.body).unwrap_or("");
        return Err(AnswerError::Server(format!(
            "HTTP {}: {}",
            response.status,
            text::printable(body, SUMMARY_MAX_CHARS, 1)
        )));
    }
    parse_answer(&response.body)
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

#[cfg(test)]
mod tests;
