//! A stand-in for the Mac mini's LLM server during tests: answers the OpenAI-style
//! chat endpoint (questions with a fixed reply, script requests with the canned
//! scripts in [`SCRIPTS`], rollback explanations by repeating the rollback they
//! were given) and `/health` with "ok". QEMU's user networking maps the
//! guest's 10.0.2.2 to the host's 127.0.0.1, so the guest reaches it there.

mod act;
mod acts;
mod query;
#[cfg(test)]
mod tests;
mod tune;

use crate::httpd::{self, HttpError};
use act::{act, act_line};
use query::query;
use serde_json::{Value, json};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;
use tune::tune;

/// Largest request body read: a chat request from the kernel is a few KiB.
const MAX_BODY: usize = 256 * 1024;
/// The line `act-stop` sends, which the mock answers only after [`SLOW_ACT`].
pub const SLOW_LINE: &str = "take your time and turn on round robin scheduling";
/// Long enough for the case to press Stop while llm waits.
const SLOW_ACT: std::time::Duration = std::time::Duration::from_secs(4);

/// The text every question gets back. Tests look for it on the console.
pub const REPLY: &str = "Hello from the mock LLM.";
/// The model's answer in words, with no script, to the script request "hello".
pub const HELLO: &str = "Hello! Ask me about this system, or tell me what to do.";
pub const MODEL: &str = "mock";
/// The `wait-reads` case's line (`kernel/src/wait_reads_tests.rs`), whose
/// `act` the mock answers only after [`WAIT_SLOW_MS`], with the CPU count's
/// call: an app's keys ask the shell for reads and changes meanwhile, and the
/// time leaves room for the host's keys after the first answer.
pub const WAIT_SLOW_LINE: &str = "take a while over the cpu count";
pub const WAIT_SLOW_MS: u64 = 8_000;
/// The same case's line whose `act` fails after [`WAIT_FAIL_MS`]: the server
/// answers an error, as a model that stops partway does.
pub const WAIT_FAIL_LINE: &str = "stop partway through the cpu count";
pub const WAIT_FAIL_MS: u64 = 6_000;
/// What every rollback explanation starts with; the rest repeats the facts'
/// `latest rollback:` line, so a test sees the ledger's facts reached the model.
pub const EXPLANATION: &str = "The mock LLM read the facts:";

pub struct MockLlm {
    pub port: u16,
    chats: Arc<AtomicUsize>,
    /// Connections accepted, whatever they asked for.
    requests: Arc<AtomicUsize>,
}

impl MockLlm {
    pub fn chats(&self) -> usize {
        self.chats.load(Ordering::SeqCst)
    }

    /// Every connection so far, so a case can show it never reached the server.
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

/// Starts the server on a free local port. It runs until xtask exits.
pub fn start() -> Result<MockLlm, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("cannot start the mock LLM server: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("mock LLM server has no address: {e}"))?
        .port();
    let chats = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(AtomicUsize::new(0));
    let (counter, accepted) = (Arc::clone(&chats), Arc::clone(&requests));
    thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            accepted.fetch_add(1, Ordering::SeqCst);
            let counter = Arc::clone(&counter);
            thread::spawn(move || {
                if let Err(error) = serve(stream, &counter) {
                    eprintln!("xtask: mock LLM: {error}");
                }
            });
        }
    });
    Ok(MockLlm {
        port,
        chats,
        requests,
    })
}

fn serve(stream: TcpStream, chats: &AtomicUsize) -> std::io::Result<()> {
    let (status, reply) = match httpd::read_request(&stream, MAX_BODY, |_| true) {
        Ok(request) => route(&request.method, &request.path, &request.body, chats),
        Err(HttpError::Io(error)) => return Err(error),
        Err(HttpError::TooLarge) => ("413 Payload Too Large", String::new()),
        Err(HttpError::Malformed(_)) => ("400 Bad Request", String::new()),
        Err(HttpError::Forbidden) => ("403 Forbidden", String::new()),
    };
    httpd::respond(&stream, status, "application/json", reply.as_bytes())
}

fn route(method: &str, path: &str, body: &[u8], chats: &AtomicUsize) -> (&'static str, String) {
    match (method, path) {
        ("GET", "/health") => ("200 OK", r#"{"status":"ok"}"#.to_string()),
        ("POST", "/v1/chat/completions") => {
            chats.fetch_add(1, Ordering::SeqCst);
            match serde_json::from_slice::<Value>(body) {
                Ok(request) if request["messages"].is_array() => {
                    let line = act_line(&request);
                    if line.as_deref() == Some(WAIT_SLOW_LINE) {
                        thread::sleep(Duration::from_millis(WAIT_SLOW_MS));
                    }
                    if line.as_deref() == Some(WAIT_FAIL_LINE) {
                        thread::sleep(Duration::from_millis(WAIT_FAIL_MS));
                        return (
                            "500 Internal Server Error",
                            r#"{"error":{"message":"the model stopped"}}"#.to_string(),
                        );
                    }
                    ("200 OK", completion(&chat_content(&request)))
                }
                _ => (
                    "400 Bad Request",
                    r#"{"error":{"message":"expected messages"}}"#.to_string(),
                ),
            }
        }
        _ => (
            "404 Not Found",
            r#"{"error":{"message":"not found"}}"#.to_string(),
        ),
    }
}

/// Script requests the mock understands: (request, first script, script after a
/// repair, summary). An empty script means the model declines to write one.
const SCRIPTS: &[(&str, &str, &str, &str)] = &[
    (
        "how many cpus are there",
        CPUS_SCRIPT,
        CPUS_SCRIPT,
        "Prints the number of CPUs.",
    ),
    (
        "show me the cpus please",
        CPUS_SCRIPT,
        CPUS_SCRIPT,
        "Prints the number of CPUs.",
    ),
    (
        "switch to round robin",
        RR_SCRIPT,
        RR_SCRIPT,
        "Switches to round-robin and prints the policy.",
    ),
    (
        "how much memory is free",
        "print(memory_free());",
        "print(memory().free_mib + \" MiB free\");",
        "Prints free memory.",
    ),
    ("hello", "", "", HELLO),
    (
        "reboot the computer",
        "",
        "",
        "There is no way to reboot from a script.",
    ),
    (
        "go back to interactive",
        "set_policy(\"interactive\");",
        "set_policy(\"interactive\");",
        "Switches to the interactive policy.",
    ),
];

const CPUS_SCRIPT: &str = "print(\"CPUs: \" + cpus());";
/// Reads the policy right before switching, so a stale metrics snapshot would show.
const RR_SCRIPT: &str = "if get_policy() != \"round-robin\" {\n    set_policy(\"round-robin\");\n}\nprint(\"policy: \" + get_policy());";

/// What the model says: the fixed reply to a question, or for a script request
/// the envelope from [`SCRIPTS`], and prose for any request not listed there.
fn chat_content(request: &Value) -> String {
    if request["messages"][0]["content"] == llmproto::explain::SYSTEM_PROMPT {
        return explanation(request);
    }
    if request["messages"][1]["content"] == llmproto::pack::MODE_ACT {
        return act(request);
    }
    if request["response_format"]["json_schema"]["name"] == "sched_tune" {
        return tune(request);
    }
    if request["messages"][0]["content"] == llmproto::query::SYSTEM_PROMPT {
        return query(request);
    }
    let is_script = request["response_format"]["json_schema"]["name"] == "rhai_script";
    if !is_script {
        return REPLY.to_string();
    }
    let messages = request["messages"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let asked = messages
        .get(1)
        .and_then(|m| m["content"].as_str())
        .and_then(|text| text.strip_prefix("Request: "))
        .and_then(|text| text.split('\n').next())
        .unwrap_or("");
    let repairing = messages.len() == 4
        && messages[3]["content"]
            .as_str()
            .is_some_and(|text| text.starts_with("That script was refused: "));
    match SCRIPTS.iter().find(|(request, ..)| *request == asked) {
        Some((_, first, repaired, summary)) => {
            let script = if repairing { repaired } else { first };
            json!({ "script": script, "summary": summary }).to_string()
        }
        None => "I am not sure what you mean.".to_string(),
    }
}

/// The server's answer to the chat request `body`, as the mock gives it over HTTP;
/// lets the host scenarios check a request without a network.
pub fn answer(body: &[u8]) -> Result<String, String> {
    let request: Value =
        serde_json::from_slice(body).map_err(|error| format!("not JSON: {error}"))?;
    Ok(completion(&chat_content(&request)))
}

/// [`EXPLANATION`] and the facts' `latest rollback:` line.
fn explanation(request: &Value) -> String {
    let facts = request["messages"][1]["content"].as_str().unwrap_or("");
    let rollback = facts
        .lines()
        .find(|line| line.starts_with("latest rollback: "))
        .unwrap_or("no rollback");
    format!("{EXPLANATION} {rollback}.")
}

fn completion(content: &str) -> String {
    json!({
        "id": "mock-1",
        "object": "chat.completion",
        "model": MODEL,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }]
    })
    .to_string()
}
