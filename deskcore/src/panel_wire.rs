//! What the shell says to `desktop` about the AI panel (P3 task 7): calls go
//! one way, so the shell takes the panel's requests and reports its state; the
//! panel never calls the shell. One request a call, as text:
//!
//! - `panel take` → `line <text>`, `stop` or `none`: the oldest request waiting;
//! - `panel state <state>` → `ok`: what the panel shows on its right, one of
//!   `idle`, `acting <call>\t<target>`, `done <line>` and `refused <reason>`.
//!   (Thinking is the panel's own, from the Enter that queued the line.)
//! - `panel ready yes` or `panel ready no` → `ok`: whether the model is warm,
//!   both of llm's prompt slots filled. Until it is, the panel says "Kai is
//!   getting ready" (founder test 2): a first request then waits for the
//!   warm-up, about 17 s after a rebuild that changed the prompt.
//! - `panel said <text>` → `ok`: what the AI said to the line the shell took
//!   last, the model's words or its script's prints, for the desk's history
//!   ([`crate::chat`]), sent before that line's state. Several lines, at most
//!   [`MAX_SAID`] bytes.
//!
//! - `appearance <kai-appearance 1 text>` → `ok` or `err <why>`: the appearance
//!   the shell just saved (P4 task 8), for `desktop` to paint in at once.
//!
//! `desktop` takes these from the shell alone. A state's text is the shell's
//! code result or reason, never the model's words, cut to one line; only
//! `panel said` carries the AI's words, and only to the history, which draws
//! them as the AI's (ADR 0005 §8, the panel's history).

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
const READY: &str = "panel ready ";

/// Whether the model is warm, as a request.
pub fn encode_ready(ready: bool) -> String {
    let word = if ready { "yes" } else { "no" };
    format!("{READY}{word}")
}

/// A `panel ready` request read back; `None` for anything else.
pub fn parse_ready(text: &str) -> Option<bool> {
    match text.strip_prefix(READY)? {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// The running model's name, from the shell's record of what it loaded
/// (`updated`'s list), never from the model's output.
const MODEL: &str = "panel model ";
/// The longest model name the pane shows, in bytes.
pub const MODEL_MAX: usize = 48;
/// What a name that is not printable ASCII shows as.
pub const MODEL_FALLBACK: &str = "model";

/// The running model's name as a request, cleaned ([`clean_model`]).
pub fn encode_model(name: &str) -> String {
    format!("{MODEL}{}", clean_model(name))
}

/// A `panel model` request read back and cleaned again at the boundary;
/// `None` for anything else.
pub fn parse_model(text: &str) -> Option<String> {
    text.strip_prefix(MODEL).map(clean_model)
}

/// A model name as the pane may show it: printable ASCII only, else
/// [`MODEL_FALLBACK`] whole; cut to [`MODEL_MAX`] bytes. The bytes are
/// checked before the trim, so a name padded with tabs or newlines is
/// "model", not trimmed: safe rather than clever.
pub fn clean_model(name: &str) -> String {
    let printable = name.bytes().all(|b| (0x20..=0x7E).contains(&b));
    let name = name.trim();
    if !printable || name.is_empty() {
        return String::from(MODEL_FALLBACK);
    }
    String::from(&name[..name.len().min(MODEL_MAX)])
}

/// What a desktop line's card came to (`shellcore::deskask`): a state, but
/// one that never replaces a newer line's thinking.
const CARD: &str = "panel card ";

/// A card's result as its request: [`encode_state`]'s, under [`CARD`].
pub fn encode_card(state: &State) -> Option<String> {
    encode_card_held(state, None)
}

/// A [`encode_card`] request read back; `None` for anything else.
pub fn parse_card(text: &str) -> Option<State> {
    parse_card_held(text).map(|(state, _)| state)
}

/// A card's result that held a send (`crate::sends`): its done line with
/// the send's log entry after a tab, as [`encode_done`] carries a line's, so
/// the pane shows the send's countdown and undo in its place.
pub fn encode_card_held(state: &State, held: Option<u64>) -> Option<String> {
    let request = encode_done(state, held)?;
    Some(format!("{CARD}{}", &request[STATE.len()..]))
}

/// A [`encode_card_held`] request read back: the state, and the held send's
/// entry if it names one.
pub fn parse_card_held(text: &str) -> Option<(State, Option<u64>)> {
    let body = text.strip_prefix(CARD)?;
    parse_done(&format!("{STATE}{body}"))
}

const SAID: &str = "panel said ";
const NOTE: &str = "panel note ";
/// The most a [`SAID`] request's text holds.
pub const MAX_SAID: usize = crate::chat::MAX_TEXT;

/// What the AI said, as its request: cut to [`MAX_SAID`] at a character.
pub fn encode_said(text: &str) -> String {
    let mut end = text.len().min(MAX_SAID);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{SAID}{}", &text[..end])
}

/// The text a [`encode_said`] request carries; `None` for anything else or
/// for one over [`MAX_SAID`].
pub fn parse_said(request: &str) -> Option<&str> {
    request
        .strip_prefix(SAID)
        .filter(|text| text.len() <= MAX_SAID)
}

/// A question code asked the line the shell took last (which target), as
/// its request: for the history as code's note, never Kai's words.
pub fn encode_note(text: &str) -> String {
    let mut end = text.len().min(MAX_SAID);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{NOTE}{}", &text[..end])
}

/// The text a [`encode_note`] request carries; `None` for anything else.
pub fn parse_note(request: &str) -> Option<&str> {
    request
        .strip_prefix(NOTE)
        .filter(|text| text.len() <= MAX_SAID)
}
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
            // Digits only, no leading zero: one entry has one spelling.
            let canonical =
                id.bytes().all(|b| b.is_ascii_digit()) && (id == "0" || !id.starts_with('0'));
            let id = id.parse::<u64>().ok().filter(|_| canonical)?;
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

    /// A model name reaches the pane printable and short: control or
    /// non-ASCII characters make it "model" whole, a long one is cut, and
    /// the line reads back as it was sent.
    #[test]
    fn a_hostile_model_name_is_cleaned_and_cut() {
        assert_eq!(
            parse_model(&encode_model("qwen3-4b-q4")),
            Some("qwen3-4b-q4".into())
        );
        assert_eq!(clean_model("\u{1b}[31mevil"), MODEL_FALLBACK);
        assert_eq!(clean_model("tab\there"), MODEL_FALLBACK);
        assert_eq!(clean_model("n\u{430}tes"), MODEL_FALLBACK);
        assert_eq!(clean_model("line\nbreak"), MODEL_FALLBACK);
        assert_eq!(clean_model("   "), MODEL_FALLBACK);
        let long = "x".repeat(100);
        assert_eq!(clean_model(&long).len(), MODEL_MAX);
        // Printable words stay words: the pane draws them in the system's
        // mono, never in the AI's look.
        assert_eq!(clean_model("Kai asks to run"), "Kai asks to run");
        // The boundary cleans what it reads, whoever sent it.
        assert_eq!(
            parse_model("panel model \u{7}bell"),
            Some(MODEL_FALLBACK.into())
        );
        assert_eq!(parse_model("panel ready yes"), None);
    }

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
        assert_eq!(parse_done("panel state done x\t+4"), None);
        assert_eq!(parse_done("panel state done x\t004"), None);
        assert_eq!(
            parse_done("panel state done x\t0").map(|d| d.1),
            Some(Some(0))
        );
    }

    #[test]
    fn a_cards_result_reads_back_and_is_not_a_state() {
        let done = State::Done {
            line: "Done: notes stopped.".into(),
        };
        let text = encode_card(&done).unwrap();
        assert_eq!(parse_card(&text), Some(done.clone()));
        assert_eq!(parse_state(&text), None);
        assert_eq!(parse_card(&encode_state(&done).unwrap()), None);
    }

    #[test]
    fn what_was_said_goes_whole_up_to_its_bound() {
        let words = "Rust is a systems language.\nIt has no garbage collector.";
        assert_eq!(parse_said(&encode_said(words)), Some(words));
        let long = "é".repeat(MAX_SAID);
        let text = encode_said(&long);
        assert!(parse_said(&text).is_some_and(|t| t.len() <= MAX_SAID));
        let over = alloc::format!("{SAID}{}", "x".repeat(MAX_SAID + 1));
        assert_eq!(parse_said(&over), None);
        assert_eq!(parse_said("panel state done x"), None);
        assert_eq!(
            parse_note(&encode_note("Which service?")),
            Some("Which service?")
        );
        assert_eq!(parse_said(&encode_note("x")), None);
        assert_eq!(parse_note(&encode_said("x")), None);
        assert_eq!(parse_state(&encode_said("x")), None);
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

    #[test]
    fn whether_the_model_is_warm_reads_back() {
        for ready in [true, false] {
            assert_eq!(parse_ready(&encode_ready(ready)), Some(ready));
        }
        assert_eq!(encode_ready(false), "panel ready no");
        for bad in [
            "panel ready",
            "panel ready maybe",
            "panel state idle",
            "ready yes",
        ] {
            assert_eq!(parse_ready(bad), None, "{bad:?}");
        }
        assert_eq!(parse_state("panel ready yes"), None);
    }
}
