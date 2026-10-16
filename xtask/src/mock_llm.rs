//! A stand-in for the Mac mini's LLM server during tests: answers the OpenAI-style
//! chat endpoint with a fixed reply and `/health` with "ok". QEMU's user networking
//! maps the guest's 10.0.2.2 to the host's 127.0.0.1, so the guest reaches it there.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

/// The text every chat request gets back. Tests look for it on the console.
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
            let looks_like_chat =
                std::str::from_utf8(body).is_ok_and(|b| b.contains("\"messages\""));
            if looks_like_chat {
                let reply = format!(
                    r#"{{"id":"mock-1","object":"chat.completion","model":"{MODEL}","choices":[{{"index":0,"message":{{"role":"assistant","content":"{REPLY}"}},"finish_reason":"stop"}}]}}"#
                );
                ("200 OK", reply)
            } else {
                (
                    "400 Bad Request",
                    r#"{"error":{"message":"expected messages"}}"#.to_string(),
                )
            }
        }
        _ => (
            "404 Not Found",
            r#"{"error":{"message":"not found"}}"#.to_string(),
        ),
    }
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
}
