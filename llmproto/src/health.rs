//! The model's state in `health`'s reply, for the Monitor app (P4, AP-1, as
//! `model_state` through comphost): the llm service ends a reply it can give
//! with `; status state=<state> tps10=<n>`, the state and the last answer's
//! decode speed in tenths of a token a second. The words before it are for
//! people and may change; this part is the contract.
//!
//! The service answers one request at a time, and loading the engine is one
//! long call, so while it loads or answers a request `health` waits: a caller
//! whose `health` times out shows [`State::Busy`]. An `err` reply has no
//! status: `no model` is [`State::NoModel`].

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
    fn speeds_round_to_tenths() {
        assert_eq!(tps10(None), 0);
        assert_eq!(tps10(Some(8.46)), 85);
        assert_eq!(tps10(Some(0.04)), 0);
        assert_eq!(tps10(Some(f64::INFINITY)), u32::MAX);
        assert_eq!(tps10(Some(f64::NAN)), 0);
    }
}
