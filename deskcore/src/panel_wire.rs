//! What the shell says to `desktop` about the AI panel (P3 task 7): calls go
//! one way, so the shell takes the panel's requests and reports its state; the
//! panel never calls the shell. One request a call, as text:
//!
//! - `panel take` → `line <text>`, `stop` or `none`: the oldest request waiting;
//! - `panel state <state>` → `ok`: what the panel shows on its right, one of
//!   `idle`, `acting <call>\t<target>`, `done <line>` and `refused <reason>`.
//!   (Thinking is the panel's own, from the Enter that queued the line.)
//!
//! - `appearance <kai-appearance 1 text>` → `ok` or `err <why>`: the appearance
//!   the shell just saved (P4 task 8), for `desktop` to paint in at once.
//!
//! `desktop` takes these from the shell alone. The texts are the shell's code
//! results and reasons, never the model's words, and are cut to one line.

use crate::panel::{Request, State};
use alloc::format;
use alloc::string::{String, ToString};

pub const TAKE: &str = "panel take";
/// Before the shell's saved appearance.
pub const APPEARANCE: &str = "appearance ";

/// The shell's saved appearance, `kai-appearance 1` text, as a request.
pub fn encode_appearance(text: &str) -> String {
    format!("{APPEARANCE}{text}")
}

/// The appearance text an [`encode_appearance`] request carries; `None` for any
/// other request. `look` checks the text itself.
pub fn parse_appearance(request: &str) -> Option<&str> {
    request.strip_prefix(APPEARANCE)
}
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

/// [`encode_state`], with the log entry a done line's undo takes back after
/// a tab (`panel state done <line>\t<id>`): the line has no tab of its own.
pub fn encode_done(state: &State, entry: Option<u64>) -> Option<String> {
    let text = encode_state(state)?;
    match (state, entry) {
        (State::Done { .. }, Some(id)) => Some(format!("{text}\t{id}")),
        _ => Some(text),
    }
}

/// A `panel state` request read back with a done line's log entry, if it
/// names one; `None` for anything else.
pub fn parse_done(text: &str) -> Option<(State, Option<u64>)> {
    let Some(body) = text
        .strip_prefix(STATE)
        .and_then(|b| b.strip_prefix("done "))
    else {
        return parse_state(text).map(|state| (state, None));
    };
    match body.rsplit_once('\t') {
        Some((line, id)) => {
            let id = id.parse::<u64>().ok()?;
            let state = parse_state(&format!("{STATE}done {line}"))?;
            Some((state, Some(id)))
        }
        None => parse_state(text).map(|state| (state, None)),
    }
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
/// `text`'s first line that is not blank, cut to [`MAX_TEXT`]; "-" if none.
fn one_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("-")
        .replace('\t', " ");
    let mut end = line.len().min(MAX_TEXT);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_appearance_goes_whole_and_nothing_else_is_taken_for_it() {
        let text = "kai-appearance 1\ntheme light\n";
        let request = super::encode_appearance(text);
        assert_eq!(super::parse_appearance(&request), Some(text));
        for other in [
            "appearance",
            "panel take",
            "panel state idle",
            " appearance x",
        ] {
            assert_eq!(super::parse_appearance(other), None, "{other:?}");
        }
    }

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
        let blank_first = State::Done {
            line: "\n42".into(),
        };
        let text = encode_state(&blank_first).unwrap();
        assert_eq!(parse_state(&text), Some(State::Done { line: "42".into() }));
        let blank = encode_state(&State::Done { line: "\n".into() }).unwrap();
        assert!(parse_state(&blank).is_some(), "{blank}");
        let long = State::Refused {
            reason: "é".repeat(300),
        };
        let text = encode_state(&long).unwrap();
        assert!(text.len() <= STATE.len() + "refused ".len() + MAX_TEXT);
        assert!(parse_state(&text).is_some());
    }

    /// #875's review: a done line names the log entry its undo takes back.
    #[test]
    fn a_done_line_carries_its_log_entry() {
        let done = State::Done {
            line: "Done: policy round-robin (was interactive).".into(),
        };
        let text = encode_done(&done, Some(12)).unwrap();
        assert_eq!(parse_done(&text), Some((done.clone(), Some(12))));
        let bare = encode_done(&done, None).unwrap();
        assert_eq!(parse_done(&bare), Some((done, None)));
        assert_eq!(parse_done("panel state idle"), Some((State::Idle, None)));
        assert_eq!(parse_done("panel state done x\tnine"), None);
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
