//! A stand-in for the Mac mini's LLM server during tests: answers the OpenAI-style
//! chat endpoint (questions with a fixed reply, script requests with the canned
//! scripts in [`SCRIPTS`]) and `/health` with "ok". QEMU's user networking maps the
//! guest's 10.0.2.2 to the host's 127.0.0.1, so the guest reaches it there.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

/// The text every question gets back. Tests look for it on the console.
pub const REPLY: &str = "Hello from the mock LLM.";
pub const MODEL: &str = "mock";

pub struct MockLlm {
    pub port: u16,
    chats: Arc<AtomicUsize>,
}

impl MockLlm {
    pub fn chats(&self) -> usize {
        self.chats.load(Ordering::SeqCst)
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
    let counter = Arc::clone(&chats);
    thread::spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            let counter = Arc::clone(&counter);
            thread::spawn(move || {
                if let Err(error) = serve(stream, &counter) {
                    eprintln!("xtask: mock LLM: {error}");
                }
            });
        }
    });
    Ok(MockLlm { port, chats })
}

fn serve(stream: TcpStream, chats: &AtomicUsize) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut content_length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    let (status, reply) = route(&request_line, &body, chats);
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
    let mut stream = stream;
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn route(request_line: &str, body: &[u8], chats: &AtomicUsize) -> (&'static str, String) {
    let mut parts = request_line.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some("GET"), Some("/health")) => ("200 OK", r#"{"status":"ok"}"#.to_string()),
        (Some("POST"), Some("/v1/chat/completions")) => {
            chats.fetch_add(1, Ordering::SeqCst);
            match serde_json::from_slice::<Value>(body) {
                Ok(request) if request["messages"].is_array() => {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn exchange(port: u16, request: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn answers_health_and_chat_and_counts_chats() {
        let server = start().unwrap();
        let health = exchange(server.port, "GET /health HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(health.starts_with("HTTP/1.1 200 OK") && health.ends_with(r#"{"status":"ok"}"#));
        let body = r#"{"model":"mock","messages":[{"role":"user","content":"hi"}]}"#;
        let chat = exchange(
            server.port,
            &format!(
                "POST /v1/chat/completions HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(chat.starts_with("HTTP/1.1 200 OK"));
        assert!(chat.contains(REPLY));
        assert_eq!(server.chats(), 1);
        let missing = exchange(server.port, "GET /nope HTTP/1.1\r\n\r\n");
        assert!(missing.starts_with("HTTP/1.1 404"));
    }

    /// The requests the kernel's `test=scripts` case makes, built and read by the
    /// same code the llm service uses.
    #[test]
    fn answers_script_requests_and_repairs() {
        use llmproto::script::{self, AnswerError, Repair};
        let ask = |text: &str, repair: Option<Repair<'_>>| {
            let request =
                serde_json::from_slice(&script::chat_request(MODEL, text, repair)).unwrap();
            script::parse_answer(completion(&chat_content(&request)).as_bytes())
        };
        assert_eq!(
            ask("how many cpus are there", None).unwrap().script,
            CPUS_SCRIPT
        );
        let broken = "print(memory_free());";
        assert_eq!(ask("how much memory is free", None).unwrap().script, broken);
        let repair = Repair {
            script: broken,
            error: "the script failed a trial run: function not found",
        };
        let repaired = ask("how much memory is free", Some(repair)).unwrap();
        assert!(repaired.script.contains("memory().free_mib"));
        assert!(matches!(
            ask("reboot the computer", None),
            Err(AnswerError::NoScript(_))
        ));
        assert_eq!(
            ask("tell me a story please", None),
            Err(AnswerError::NoEnvelope)
        );
        let question =
            serde_json::from_slice(&llmproto::openai::chat_request(MODEL, None, "hi", 8)).unwrap();
        assert_eq!(chat_content(&question), REPLY);
    }
}
