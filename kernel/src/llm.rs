//! LLM client: OpenAI-compatible chat completions over HTTP to the server named on
//! the kernel command line (`llm=IP:PORT`, optional `model=NAME`), normally the Mac
//! mini on the LAN. Must be called from a kernel thread.

use crate::{hpet, log, net};
use alloc::format;
use alloc::string::{String, ToString};
use core::net::Ipv4Addr;
use llmproto::http::{self, ParseError};
use llmproto::openai;
use spin::Once;

const DEFAULT_MODEL: &str = "qwen3-8b";
const SYSTEM_PROMPT: &str = "You are the assistant built into Kernel AI, a small experimental \
                             operating system. Answer in a few short sentences.";
const MAX_TOKENS: u32 = 256;
const CHAT_TIMEOUT_MS: u64 = 120_000;
const HEALTH_TIMEOUT_MS: u64 = 5_000;

struct Config {
    server: (Ipv4Addr, u16),
    model: String,
}

static CONFIG: Once<Config> = Once::new();

/// Reads `llm=IP:PORT` and `model=NAME` from the kernel command line.
pub fn configure(cmdline: &str) {
    let mut server = None;
    let mut model = DEFAULT_MODEL;
    for arg in cmdline.split_whitespace() {
        if let Some(value) = arg.strip_prefix("llm=") {
            server = parse_server(value);
            if server.is_none() {
                log!("llm: ignoring llm={value}: expected IPv4:PORT");
            }
        } else if let Some(value) = arg.strip_prefix("model=") {
            model = value;
        }
    }
    if let Some(server) = server {
        CONFIG.call_once(|| Config {
            server,
            model: model.to_string(),
        });
    }
}

fn parse_server(text: &str) -> Option<(Ipv4Addr, u16)> {
    let (host, port) = text.rsplit_once(':')?;
    Some((host.parse().ok()?, port.parse().ok().filter(|&p| p > 0)?))
}

/// The configured server and model, if any.
pub fn target() -> Option<(Ipv4Addr, u16, &'static str)> {
    CONFIG
        .get()
        .map(|config| (config.server.0, config.server.1, config.model.as_str()))
}

fn config() -> Result<&'static Config, String> {
    CONFIG
        .get()
        .ok_or_else(|| "no LLM server configured (boot with llm=IP:PORT)".to_string())
}

fn host_header(config: &Config) -> String {
    format!("{}:{}", config.server.0, config.server.1)
}

/// The response is complete once it parses, or once it can never parse.
fn complete(response: &[u8], closed: bool) -> bool {
    !matches!(
        http::parse_response(response, closed),
        Err(ParseError::Incomplete)
    )
}

fn round_trip(config: &Config, request: &[u8], timeout_ms: u64) -> Result<http::Response, String> {
    let raw = net::exchange(config.server, request, timeout_ms, complete).map_err(String::from)?;
    http::parse_response(&raw, true).map_err(|error| format!("bad HTTP response: {error:?}"))
}

/// Checks the server answers `GET /health`; returns the HTTP status and round trip.
pub fn health() -> Result<(u16, u64), String> {
    let config = config()?;
    let started = hpet::nanos();
    let response = round_trip(
        config,
        &http::get(&host_header(config), "/health"),
        HEALTH_TIMEOUT_MS,
    )?;
    Ok((response.status, (hpet::nanos() - started) / 1_000_000))
}

/// Sends one user message and returns the assistant's reply.
pub fn ask(prompt: &str) -> Result<String, String> {
    let config = config()?;
    let body = openai::chat_request(&config.model, Some(SYSTEM_PROMPT), prompt, MAX_TOKENS);
    let request = http::post_json(&host_header(config), openai::CHAT_PATH, None, &body);
    let response = round_trip(config, &request, CHAT_TIMEOUT_MS)?;
    if response.status != 200 {
        let snippet = core::str::from_utf8(&response.body)
            .unwrap_or("")
            .chars()
            .take(120);
        return Err(format!(
            "server answered HTTP {}: {}",
            response.status,
            snippet.collect::<String>()
        ));
    }
    openai::parse_chat_response(&response.body)
        .map_err(|error| format!("bad chat reply: {error:?}"))
}

/// M6 exit test, network half: the server answers its health check and a chat request.
pub fn self_test() -> Result<(), &'static str> {
    if !net::is_up() {
        return Err("llm: the network card did not come up");
    }
    let (status, rtt) = health().map_err(|error| {
        log!("llm: health check failed: {error}");
        "llm: health check failed"
    })?;
    log!("llm: GET /health -> HTTP {status} in {rtt} ms");
    if status != 200 {
        return Err("llm: health check did not return 200");
    }
    let started = hpet::nanos();
    let reply = ask("Say hello.").map_err(|error| {
        log!("llm: chat failed: {error}");
        "llm: chat request failed"
    })?;
    log!(
        "llm: reply in {} ms: {reply}",
        (hpet::nanos() - started) / 1_000_000
    );
    Ok(())
}
