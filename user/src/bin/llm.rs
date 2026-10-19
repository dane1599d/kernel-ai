//! The LLM client service. It takes `ask <prompt>`, `script ...` (see
//! `llmproto::script`), `explain <facts>` (see `llmproto::explain`) and `health`
//! requests from the kernel terminal and the shell, talks OpenAI-compatible chat
//! JSON over HTTP to the LLM server, and answers `ok <text>` or `err <text>`. The
//! kernel decides where the bytes go (the server on its command line); this program
//! builds and parses them.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use kabi::HttpRequest;
use llmproto::explain;
use llmproto::script::{self, AnswerError, ScriptRequest};
use llmproto::text::printable;
use llmproto::{http, openai};
use user::{arg, args, println, sys};

user::entry!(main);

const DEFAULT_MODEL: &str = "qwen3-8b";
const SYSTEM_PROMPT: &str = "You are the assistant built into Kernel AI, a small experimental \
                             operating system. Answer in a few short sentences.";
const MAX_TOKENS: u32 = 256;
const CHAT_TIMEOUT_MS: u64 = 120_000;
const HEALTH_TIMEOUT_MS: u64 = 5_000;
const MAX_REPLY_CHARS: usize = 4096;
const MAX_REPLY_LINES: usize = 64;
const MAX_ERROR_CHARS: usize = 160;

struct Config {
    /// `IP:PORT` of the server, for the Host header only.
    host: Option<String>,
    model: String,
}

fn main() -> i32 {
    let args = args();
    let config = Config {
        host: arg(&args, "host").map(ToString::to_string),
        model: arg(&args, "model").unwrap_or(DEFAULT_MODEL).to_string(),
    };
    match &config.host {
        Some(host) => println!(
            "up (restart {}); server {host}, model {}",
            arg(&args, "restart").unwrap_or("0"),
            config.model
        ),
        None => println!("up; no LLM server configured (boot with llm=IP:PORT)"),
    }
    let mut request = vec![0u8; kabi::MAX_MESSAGE];
    let mut response = vec![0u8; http::MAX_RESPONSE];
    loop {
        let mut caller = 0;
        let len = match sys::svc_recv(&mut request, 0, &mut caller) {
            Ok(len) => len.min(request.len()),
            Err(kabi::Error::TimedOut) => continue,
            Err(error) => {
                println!("svc_recv failed: {error:?}");
                sys::sleep_ms(100);
                continue;
            }
        };
        let text = core::str::from_utf8(&request[..len]).unwrap_or("");
        let mut reply = match handle(&config, text, &mut response) {
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

fn handle(config: &Config, text: &str, response: &mut [u8]) -> Result<String, String> {
    let (command, rest) = text.split_once(' ').unwrap_or((text, ""));
    let host = config
        .host
        .as_deref()
        .ok_or("no LLM server configured (boot with llm=IP:PORT)")?;
    match command {
        "health" => health(host, response),
        "ask" if !rest.trim().is_empty() => ask(config, host, rest.trim(), response),
        "script" => write_script(config, host, text, response),
        "explain" if rest.len() > explain::MAX_FACTS => Err(String::from("the facts are too long")),
        "explain" if !rest.trim().is_empty() => explain(config, host, rest, response),
        _ => Err(format!("unknown request {command:?}")),
    }
}

/// Asks the model for a script. The answer goes back framed by length, exactly as
/// the model wrote it after the text checks; the shell checks it again and runs it.
fn write_script(
    config: &Config,
    host: &str,
    text: &str,
    response: &mut [u8],
) -> Result<String, String> {
    let request = ScriptRequest::decode(text).ok_or("server malformed script request")?;
    let repair = request
        .repair
        .as_ref()
        .map(|(script, error)| script::Repair { script, error });
    let body = script::chat_request(&config.model, &request.request, repair);
    let http_request = http::post_json(host, openai::CHAT_PATH, None, &body);
    let timeout = request.timeout_ms.min(script::SERVICE_TIMEOUT_MS);
    exchange(&http_request, timeout, response)
        .map_err(AnswerError::Server)
        .and_then(|reply| script::answer_from(&reply))
        .map(|answer| script::encode_answer(&answer))
        .map_err(|error| script::encode_error(&error))
}

fn exchange(
    request: &[u8],
    timeout_ms: u64,
    response: &mut [u8],
) -> Result<http::Response, String> {
    let call = HttpRequest {
        request_ptr: request.as_ptr() as u64,
        request_len: request.len() as u64,
        response_ptr: response.as_mut_ptr() as u64,
        response_cap: response.len() as u64,
        timeout_ms,
    };
    let len = sys::http(&call).map_err(|error| format!("network: {error:?}"))?;
    http::parse_response(&response[..len.min(response.len())], true)
        .map_err(|error| format!("bad HTTP response: {error:?}"))
}

fn health(host: &str, response: &mut [u8]) -> Result<String, String> {
    let started = sys::time_ns();
    let reply = exchange(&http::get(host, "/health"), HEALTH_TIMEOUT_MS, response)?;
    let rtt_ms = (sys::time_ns() - started) / 1_000_000;
    Ok(format!("HTTP {} in {rtt_ms} ms", reply.status))
}

fn ask(config: &Config, host: &str, prompt: &str, response: &mut [u8]) -> Result<String, String> {
    let body = openai::chat_request(&config.model, Some(SYSTEM_PROMPT), prompt, MAX_TOKENS);
    let request = http::post_json(host, openai::CHAT_PATH, None, &body);
    let reply = exchange(&request, CHAT_TIMEOUT_MS, response)?;
    if reply.status != 200 {
        let body = core::str::from_utf8(&reply.body).unwrap_or("");
        return Err(format!(
            "server answered HTTP {}: {}",
            reply.status,
            printable(body, MAX_ERROR_CHARS, 1)
        ));
    }
    openai::parse_chat_response(&reply.body)
        .map(|text| printable(&text, MAX_REPLY_CHARS, MAX_REPLY_LINES))
        .map_err(|error| format!("bad chat reply: {error:?}"))
}

/// Asks the model to explain a rollback from the kernel's facts; plain prose back.
fn explain(
    config: &Config,
    host: &str,
    facts: &str,
    response: &mut [u8],
) -> Result<String, String> {
    let body = explain::chat_request(&config.model, facts);
    let request = http::post_json(host, openai::CHAT_PATH, None, &body);
    let reply = exchange(&request, CHAT_TIMEOUT_MS, response)?;
    explain::answer_from(&reply)
}
