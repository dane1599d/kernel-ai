//! The kernel's side of the LLM: which server to use (`llm=IP:PORT`, optional
//! `model=NAME` on the kernel command line), and calls to the ring-3 `llm` service,
//! which runs the model on the `kai-model` disk itself or builds and parses the
//! HTTP and JSON for the server. The kernel only carries bytes to the one server
//! named here.

use crate::{hpet, ipc, log};
use alloc::format;
use alloc::string::{String, ToString};
use core::net::Ipv4Addr;
use llmproto::chat::NO_MODEL;
use llmproto::text::printable;
use spin::Once;

const DEFAULT_MODEL: &str = "qwen3-8b";
/// Keeps the llm service's argument string under kabi::MAX_ARGS.
const MAX_MODEL_LEN: usize = 128;
/// The service has 120 s for the chat itself; this adds time for it to come up.
const ASK_TIMEOUT_MS: u64 = 150_000;
const HEALTH_TIMEOUT_MS: u64 = 20_000;
const MAX_REPLY_CHARS: usize = 4096;
const MAX_REPLY_LINES: usize = 64;

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
            server = crate::net::parse_endpoint(value);
            if server.is_none() {
                log!("llm: ignoring llm={value}: expected IPv4:PORT");
            }
        } else if let Some(value) = arg.strip_prefix("model=") {
            if value.len() <= MAX_MODEL_LEN {
                model = value;
            } else {
                log!("llm: ignoring a model name longer than {MAX_MODEL_LEN} bytes");
            }
        }
    }
    if let Some(server) = server {
        CONFIG.call_once(|| Config {
            server,
            model: model.to_string(),
        });
    }
}

/// The configured server and model, if any.
pub fn target() -> Option<(Ipv4Addr, u16, &'static str)> {
    CONFIG
        .get()
        .map(|config| (config.server.0, config.server.1, config.model.as_str()))
}

/// The plan line for the `llm` service. With a model loaded, or reserved and
/// waiting for the model store to fill it (P2 task 10), it also gets
/// `cap::MODEL`, `cap::THREADS`, and pages for a pool of workers (L6) and for the
/// engine (its KV cache and tokenizer, `kabi::model::ENGINE_PAGES`); the model's
/// window is the kernel's; it keeps `net` for an LLM server named as well (CI's
/// test double), and names its burst limit, so a long stretch of start-up
/// work between two sleeps is not taken for a runaway. The model name is one
/// command-line word, so it holds no whitespace and cannot add arguments of its
/// own; control characters are refused as well.
pub fn plan_line() -> String {
    let model = crate::model::get().is_some() || crate::model::fill::pending();
    let mut line = match model {
        true => format!(
            "llm caps=serve+net+model+threads burst_ms=10000 pages={}",
            crate::model::holder_pages()
        ),
        false => format!(
            "llm caps=serve+net burst_ms=10000 pages={}",
            crate::model::HOLDER_PAGES
        ),
    };
    let mut passed = String::new();
    if let Some((ip, port, model)) = target() {
        passed.push_str(&format!(" host={ip}:{port}"));
        if model.chars().all(|c| !c.is_whitespace() && !c.is_control()) {
            passed.push_str(&format!(" model={model}"));
        } else {
            log!("llm: the model name has control characters; using the service default");
        }
    }
    // The model's SHA-256 as the boot names it (hex digits, one word), for llm's
    // KV files (S10-3), while llm's arguments still fit with the supervisor's
    // `restart=N`; without it llm keys them by a sampled hash.
    if let Some(sha) = crate::model::sha256() {
        let word = format!(" model_sha256={}", kabi::model::sha256_hex(sha));
        match passed.len() + word.len() + RESTART_ROOM <= kabi::MAX_ARGS {
            true => passed.push_str(&word),
            false => log!("llm: no room for model_sha256= beside the server's arguments"),
        }
    }
    line.push_str(&passed);
    line
}

/// What the supervisor adds to a service's arguments: ` restart=N`.
const RESTART_ROOM: usize = 32;

/// Asks the `llm` service; returns its reply or why there is none.
fn request(text: &str, timeout_ms: u64) -> Result<String, String> {
    let reply = ipc::call("llm", text.as_bytes(), timeout_ms)
        .map_err(|error| format!("llm service: {error:?}"))?;
    let reply = core::str::from_utf8(&reply).unwrap_or("err reply is not UTF-8");
    // Sanitised again here: the service is just another ring-3 program.
    let clean = printable(reply, MAX_REPLY_CHARS + 4, MAX_REPLY_LINES);
    match clean.split_once(' ').unwrap_or((clean.as_str(), "")) {
        ("ok", text) => Ok(text.to_string()),
        ("err", text) => Err(text.to_string()),
        _ => Err("llm service: malformed reply".to_string()),
    }
}

/// Whether llm has the act prefix and the script prompt in their KV slots, so a
/// request pays neither (`test=latency` waits for it before its warm requests).
pub fn warm() -> bool {
    request("warm", HEALTH_TIMEOUT_MS).is_ok_and(|reply| reply == "yes")
}

/// Whether anything can answer: a model from the `kai-model` disk or a server.
pub fn available() -> bool {
    target().is_some() || crate::model::get().is_some()
}

/// Sends one user message and returns the assistant's reply. With neither a model
/// nor a server it says how to set one up, at once.
pub fn ask(prompt: &str) -> Result<String, String> {
    if !available() {
        return Err(String::from(NO_MODEL));
    }
    request(&format!("ask {prompt}"), ASK_TIMEOUT_MS)
}

/// The llm service's report on the local model for `:model`: its name, licence and
/// shape from the file, and one timed test prompt.
pub fn model_report() -> Result<String, String> {
    if ipc::server_of("llm").is_none() {
        return Err(String::from("the llm service is not running"));
    }
    request("model", ASK_TIMEOUT_MS)
}

/// The model's explanation of a rollback from `facts`, from the local model or the
/// server, or why there is none. With neither, or no llm service running, it says
/// so at once, rather than waiting for a service that will not come.
pub fn explain(facts: &str) -> Result<String, String> {
    if !available() {
        return Err(String::from(NO_MODEL));
    }
    if ipc::server_of("llm").is_none() {
        return Err(String::from("the llm service is not running"));
    }
    log!("(asking the LLM...)");
    request(&format!("explain {facts}"), ASK_TIMEOUT_MS)
}

/// Checks the server answers `GET /health`, and what model the service mapped,
/// through the service. Its first health hashes the whole model, so the wait
/// grows by a second for every 8 MiB of it.
pub fn health() -> Result<String, String> {
    let hashing_ms = crate::model::get().map_or(0, |model| model.len() / (8 << 20) * 1000);
    request("health", HEALTH_TIMEOUT_MS + hashing_ms)
}

/// M6 exit test, network half, now through the ring-3 service: the server answers
/// a health check and a chat request, and the service that answered is a process.
pub fn self_test() -> Result<(), &'static str> {
    if !crate::net::is_up() {
        return Err("llm: the network card did not come up");
    }
    let started = hpet::nanos();
    let status = health().map_err(|error| {
        log!("llm: health check failed: {error}");
        "llm: health check failed"
    })?;
    log!("llm: health via the ring-3 service: {status}");
    if !status.starts_with("HTTP 200") {
        return Err("llm: health check did not return 200");
    }
    let server = ipc::server_of("llm").ok_or("llm: no process serves llm")?;
    let reply = ask("Say hello.").map_err(|error| {
        log!("llm: chat failed: {error}");
        "llm: chat request failed"
    })?;
    log!(
        "llm: reply from pid {server} in {} ms: {}",
        (hpet::nanos() - started) / 1_000_000,
        printable(&reply, 160, 1)
    );
    Ok(())
}
