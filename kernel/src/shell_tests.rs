//! M8a self-test (`test=shell`): the ring-3 Rhai shell, called exactly as the
//! terminal calls it, answers a fixed list of lines. Replies are checked in the
//! kernel, and the same shell process must answer every line.

use crate::{ipc, log, threads};
use alloc::string::String;

pub enum Expect {
    Exact(&'static str),
    StartsWith(&'static str),
    Contains(&'static str),
}

use Expect::{Contains, Exact, StartsWith};

const LINES: &[(&str, Expect)] = &[
    ("let x = 40", Exact("ok ")),
    ("fn add(a, b) { a + b }", Exact("ok ")),
    ("add(x, 2)", Exact("ok 42")),
    ("print(\"once\"); 1", Exact("ok once\n1")),
    // Earlier lines do not run again.
    ("x + 1", Exact("ok 41")),
    ("mem", Contains("MiB free")),
    ("cpus() > 0", Exact("ok true")),
    ("policy", Exact("ok scheduling policy: interactive")),
    // Bounded lines: they end in an error and the shell keeps its state.
    (
        "loop { }",
        Exact("err stopped: time limit (2000 ms) reached"),
    ),
    ("fn deep(n) { deep(n + 1) } deep(0)", StartsWith("err ")),
    (
        "for i in 0..2000 { print(\"line \" + i) }",
        Contains("(output truncated:"),
    ),
    ("x", Exact("ok 40")),
    ("ask hello", Exact("ok Hello from the mock LLM.")),
    ("reset", Exact("ok variables and functions cleared")),
    ("x", StartsWith("err ")),
];

const START_TIMEOUT_MS: u64 = 60_000;
const LINE_TIMEOUT_MS: u64 = 30_000;

/// Waits for the shell and the llm service, and returns the shell's pid.
pub fn start() -> Result<u64, &'static str> {
    for _ in 0..START_TIMEOUT_MS / 10 {
        if ipc::server_of("shell").is_some() && ipc::server_of("llm").is_some() {
            break;
        }
        threads::sleep_ms(10);
    }
    ipc::server_of("shell").ok_or("shell: never came up")
}

/// Sends each line to the shell and checks its reply.
pub fn expect_lines(lines: &[(&str, Expect)]) -> Result<(), &'static str> {
    for (line, expect) in lines {
        let reply = ipc::call("shell", line.as_bytes(), LINE_TIMEOUT_MS).map_err(|error| {
            log!("shell: no reply to {line:?}: {error:?}");
            "shell: a line got no reply"
        })?;
        let reply = String::from_utf8_lossy(&reply);
        let ok = match expect {
            Exact(text) => reply == *text,
            StartsWith(text) => reply.starts_with(text),
            Contains(text) => reply.starts_with("ok ") && reply.contains(text),
        };
        if !ok {
            log!("shell: {line:?} answered {reply:?}");
            return Err("shell: a line got the wrong answer");
        }
    }
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    let shell = start()?;
    expect_lines(LINES)?;
    if ipc::server_of("shell") != Some(shell) {
        return Err("shell: a different process answered by the end");
    }
    log!(
        "shell: {} lines answered correctly by pid {shell}",
        LINES.len()
    );
    Ok(())
}
