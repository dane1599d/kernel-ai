//! What the shell says to `desktop` about the AI panel (P3 task 7): calls go
//! one way, so the shell takes the panel's requests and reports its state; the
//! panel never calls the shell. One request a call, as text:
//!
//! - `panel take` → `line <text>`, `stop` or `none`: the oldest request waiting;
//! - `panel state <state>` → `ok`: what the panel shows on its right, one of
//!   `idle`, `acting <call>\t<target>`, `done <line>` and `refused <reason>`.
//!   (Thinking is the panel's own, from the Enter that queued the line.)
//!
//! `desktop` takes these from the shell alone. The texts are the shell's code
//! results and reasons, never the model's words, and are cut to one line.

use crate::panel::{Request, State};
use alloc::format;
use alloc::string::{String, ToString};

pub const TAKE: &str = "panel take";
const STATE: &str = "panel state ";
/// The longest text a state carries: the panel shows one line.
pub const MAX_TEXT: usize = 200;

/// The answer to [`TAKE`].
pub fn encode_take(request: Option<&Request>) -> String {
    match request {
        Some(Request::Line(text)) => format!("line {text}"),
        Some(Request::Stop) => String::from("stop"),
        None => String::from("none"),
    }
}

/// [`encode_take`] read back; `None` for anything else.
pub fn parse_take(text: &str) -> Option<Option<Request>> {
    match text {
        "none" => Some(None),
        "stop" => Some(Some(Request::Stop)),
        _ => text
            .strip_prefix("line ")
            .map(|line| Some(Request::Line(line.to_string()))),
    }
}

/// A state the shell may report, as its request. `Thinking` is the panel's
/// own, so it has none.
pub fn encode_state(state: &State) -> Option<String> {
    let body = match state {
        State::Idle => String::from("idle"),
        State::Thinking { .. } => return None,
        State::Acting { call, target } => {
            format!("acting {}\t{}", one_line(call), one_line(target))
        }
        State::Done { line } => format!("done {}", one_line(line)),
        State::Refused { reason } => format!("refused {}", one_line(reason)),
    };
    Some(format!("{STATE}{body}"))
}

/// A `panel state` request read back; `None` for anything else.
pub fn parse_state(text: &str) -> Option<State> {
    let body = text.strip_prefix(STATE)?;
    let (word, rest) = body.split_once(' ').unwrap_or((body, ""));
    let fits = rest.len() <= MAX_TEXT * 2 + 1 && !rest.contains('\n');
    if !fits {
        return None;
    }
    match word {
        "idle" if rest.is_empty() => Some(State::Idle),
        "acting" => {
            let (call, target) = rest.split_once('\t')?;
            Some(State::Acting {
                call: call.to_string(),
                target: target.to_string(),
            })
        }
        "done" if !rest.is_empty() => Some(State::Done {
            line: rest.to_string(),
        }),
        "refused" if !rest.is_empty() => Some(State::Refused {
            reason: rest.to_string(),
        }),
        _ => None,
    }
}

/// The first line of `text`, tabs made spaces, at most [`MAX_TEXT`] bytes.
fn one_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").replace('\t', " ");
    let mut end = line.len().min(MAX_TEXT);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_states_read_back() {
        for request in [
            None,
            Some(Request::Stop),
            Some(Request::Line("hi there".into())),
        ] {
            assert_eq!(parse_take(&encode_take(request.as_ref())), Some(request));
        }
        for state in [
            State::Idle,
            State::Acting {
                call: "set_policy(\"round-robin\")".into(),
                target: "policy".into(),
            },
            State::Done {
                line: "Done: policy round-robin (was interactive). undo".into(),
            },
            State::Refused {
                reason: "Refused tune: it is never run".into(),
            },
        ] {
            let text = encode_state(&state).unwrap();
            assert_eq!(parse_state(&text), Some(state), "{text}");
        }
        assert_eq!(encode_state(&State::Thinking { since_ms: 1 }), None);
    }

    #[test]
    fn a_state_carries_one_short_line() {
        let done = State::Done {
            line: "first\nsecond".into(),
        };
        assert_eq!(encode_state(&done).unwrap(), "panel state done first");
        let long = State::Refused {
            reason: "é".repeat(300),
        };
        let text = encode_state(&long).unwrap();
        assert!(text.len() <= STATE.len() + "refused ".len() + MAX_TEXT);
        assert!(parse_state(&text).is_some());
    }

    #[test]
    fn anything_else_is_refused() {
        for bad in [
            "panel state",
            "panel state idle now",
            "panel state done",
            "panel state acting no-tab",
            "panel state thinking",
            "panel state done a\nb",
            "panel take",
            "state done x",
        ] {
            assert_eq!(parse_state(bad), None, "{bad:?}");
        }
        assert_eq!(parse_take("maybe"), None);
    }
}
