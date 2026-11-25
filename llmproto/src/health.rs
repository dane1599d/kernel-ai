//! The model's state in `health`'s reply, for the Monitor app (P4, AP-1, as
//! `model_state` through comphost): the llm service ends a reply it can give
//! with `; status state=<state> tps10=<n>`, the state and the last answer's
//! decode speed in tenths of a token a second. The words before it are for
//! people and may change; this part is the contract.
//!
//! The service answers one request at a time, and loading the engine is one
//! long call, so while it loads or answers a request `health` waits: a caller
//! whose `health` times out shows [`State::Busy`]. An `err` reply has no
//! status, but for one: a server whose health failed beside a local model
//! still ends with the local status ([`reply`]). The no-model error is
//! [`State::NoModel`] ([`state_of`]).

use crate::text::printable;
use alloc::format;
use alloc::string::String;

/// What the model is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Waiting for the model store to fill the model (P2 task 10).
    Waiting,
    /// Caching its prompts after start or after its history changed; it
    /// answers, a little slower.
    Warming,
    /// Ready to answer.
    Ready,
    /// Only an LLM server answers; no local model.
    Remote,
    /// A model is mapped and cannot run.
    Failed,
    /// No model and no server: what an `err no model` reply means.
    NoModel,
    /// `health` did not answer in time: loading, or answering a request.
    Busy,
}

impl State {
    pub const ALL: [State; 7] = [
        State::Waiting,
        State::Warming,
        State::Ready,
        State::Remote,
        State::Failed,
        State::NoModel,
        State::Busy,
    ];

    pub const fn word(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Warming => "warming",
            State::Ready => "ready",
            State::Remote => "remote",
            State::Failed => "failed",
            State::NoModel => "none",
            State::Busy => "busy",
        }
    }

    pub fn from_word(word: &str) -> Option<State> {
        State::ALL.into_iter().find(|state| state.word() == word)
    }
}

/// A `health` reply's machine-readable part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// The last answer's decode speed in tokens a second times ten; 0 before
    /// the first answer, and for a server.
    pub tps10: u32,
}

/// What precedes the status in a reply.
const LEAD: &str = "status ";

impl Status {
    /// `status state=ready tps10=85`.
    pub fn line(self) -> String {
        format!("{LEAD}state={} tps10={}", self.state.word(), self.tps10)
    }

    /// The status a `health` reply's text ends with, `ok ` taken off or not.
    pub fn of(health: &str) -> Option<Status> {
        let at = health.rfind(LEAD)?;
        if at != 0 && !health[..at].ends_with("; ") {
            return None;
        }
        let mut words = health[at + LEAD.len()..].split(' ');
        let state = State::from_word(words.next()?.strip_prefix("state=")?)?;
        let tps10 = words.next()?.strip_prefix("tps10=")?.parse().ok()?;
        words.next().is_none().then_some(Status { state, tps10 })
    }
}

/// How long the service waits for its LLM server's `GET /health`. A caller
/// that must hear the local model's state waits longer than this.
pub const SERVER_HEALTH_MS: u64 = 5_000;

/// The longest part of a server's error a reply keeps when the local model's
/// words and status follow it.
const SERVER_ERROR_CHARS: usize = 64;

/// `health`'s reply, before the `ok `/`err ` the service puts first: the LLM
/// server's health when there is a server, then `local`, the model's and the
/// engine's words, then `status`. With neither it is the error
/// [`crate::chat::NO_MODEL`]. When the server's health failed and there is a
/// local side, the reply is an error that still ends with the status, cut to
/// `max_chars` (the service's cap on an error line) with the status and the
/// model's words kept: the local model's state is what a caller reads, and
/// `updated` judges a model on trial by it.
pub fn reply(
    server: Option<Result<String, String>>,
    local: Option<String>,
    status: Status,
    max_chars: usize,
) -> Result<String, String> {
    let line = status.line();
    match (server, local) {
        (None, None) => Err(String::from(crate::chat::NO_MODEL)),
        (None, Some(local)) => Ok(format!("{local}; {line}")),
        (Some(Ok(server)), Some(local)) => Ok(format!("{server}; {local}; {line}")),
        (Some(Ok(server)), None) => Ok(format!("{server}; {line}")),
        (Some(Err(error)), None) => Err(error),
        (Some(Err(error)), Some(local)) => {
            let error = printable(&error, SERVER_ERROR_CHARS, 1);
            let room = max_chars.saturating_sub(line.len() + 2);
            let words = printable(&format!("{error}; {local}"), room, 1);
            Err(format!("{words}; {line}"))
        }
    }
}

/// The model's state a `health` reply gives, `ok ` or `err ` first: its
/// status's, or [`State::NoModel`] for the error the service gives with no
/// model and no server; none for anything else.
pub fn state_of(reply: &str) -> Option<State> {
    if let Some(status) = Status::of(reply) {
        return Some(status.state);
    }
    let why = reply.strip_prefix("err ")?;
    why.starts_with(crate::chat::NO_MODEL)
        .then_some(State::NoModel)
}

/// Tokens a second as [`Status::tps10`].
pub fn tps10(tokens_per_s: Option<f64>) -> u32 {
    tokens_per_s.map_or(0, |rate| {
        let tenths = rate * 10.0 + 0.5;
        if tenths >= u32::MAX as f64 {
            u32::MAX
        } else if tenths > 0.0 {
            tenths as u32
        } else {
            0
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_ends_the_reply_and_reads_back() {
        let status = Status {
            state: State::Ready,
            tps10: 85,
        };
        let reply = format!(
            "ok model 12 bytes fnv1a64 00ff; local model x; {}",
            status.line()
        );
        assert_eq!(Status::of(&reply), Some(status));
        assert_eq!(Status::of(&status.line()), Some(status));
        for state in State::ALL {
            let line = Status { state, tps10: 0 }.line();
            assert_eq!(Status::of(&line).map(|s| s.state), Some(state));
        }
    }

    #[test]
    fn anything_else_has_no_status() {
        for text in [
            "err no model",
            "ok local model x",
            "ok x; status state=ready",
            "ok x; status state=sleepy tps10=1",
            "ok x; status state=ready tps10=-1",
            "ok x; status state=ready tps10=1 more",
            "ok x status state=ready tps10=1",
        ] {
            assert_eq!(Status::of(text), None, "{text}");
        }
    }

    #[test]
    fn a_reply_ends_with_the_status_and_a_failing_server_does_not_hide_it() {
        let ready = Status {
            state: State::Ready,
            tps10: 85,
        };
        let local = || {
            Some(String::from(
                "model 12 bytes fnv1a64 00000000000000ff; local model x",
            ))
        };
        let server = |r: Result<&str, &str>| Some(r.map(String::from).map_err(String::from));
        let line = ready.line();
        assert_eq!(
            reply(server(Ok("HTTP 200 in 4 ms")), local(), ready, 160),
            Ok(format!("HTTP 200 in 4 ms; {}; {line}", local().unwrap()))
        );
        assert_eq!(
            reply(None, local(), ready, 160),
            Ok(format!("{}; {line}", local().unwrap()))
        );
        let remote = Status {
            state: State::Remote,
            tps10: 0,
        };
        assert_eq!(
            reply(server(Ok("HTTP 200 in 4 ms")), None, remote, 160),
            Ok(format!("HTTP 200 in 4 ms; {}", remote.line()))
        );
        assert_eq!(
            reply(None, None, ready, 160),
            Err(String::from(crate::chat::NO_MODEL))
        );
        assert_eq!(
            reply(server(Err("network: refused")), None, remote, 160),
            Err(String::from("network: refused"))
        );
        // The server's health failed and the local model is there: an error
        // that still ends with the local state, within the error line's cap,
        // with the model's words kept however long the error is.
        for error in ["network: refused", &"x".repeat(400), "line one\nline two"] {
            let Err(text) = reply(server(Err(error)), local(), ready, 160) else {
                panic!("an error, for {error}");
            };
            assert!(text.chars().count() <= 160, "{text}");
            assert_eq!(Status::of(&text), Some(ready), "{text}");
            assert!(
                text.contains("; model 12 bytes fnv1a64 00000000000000ff"),
                "{text}"
            );
            assert_eq!(state_of(&format!("err {text}")), Some(State::Ready));
        }
    }

    #[test]
    fn the_no_model_error_is_no_model() {
        let no_model = format!("err {}", crate::chat::NO_MODEL);
        assert_eq!(state_of(&no_model), Some(State::NoModel));
        assert_eq!(
            state_of("ok x; status state=busy tps10=0"),
            Some(State::Busy)
        );
        for other in ["err network: refused", "ok x", "err no"] {
            assert_eq!(state_of(other), None, "{other}");
        }
    }

    #[test]
    fn speeds_round_to_tenths() {
        assert_eq!(tps10(None), 0);
        assert_eq!(tps10(Some(8.46)), 85);
        assert_eq!(tps10(Some(0.04)), 0);
        assert_eq!(tps10(Some(f64::INFINITY)), u32::MAX);
        assert_eq!(tps10(Some(f64::NAN)), 0);
    }
}
