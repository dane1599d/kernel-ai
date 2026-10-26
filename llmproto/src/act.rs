//! The `act` request (ADR 0002 D4): the user's line, and the State line the OS
//! built, answered by one tagged call, `{"name":..,"arguments":{..}}`, a row of
//! the registry or one of the pseudo-actions `say`, `clarify` and
//! `script(code, lang)`. The prompt is ADR 0010's layout: the pack, the `act`
//! MODE turn, then the request with its State line, with no `/no_think`
//! (S10-1's arm A0 adopted it, #33). The shell owns what a call may do; this
//! module only frames it and checks that it is a call.

use crate::chat::{self, Message, Role};
use crate::pack::{self, Format, MODE_ACT};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde_json::{Map, Value};

pub mod history;

pub use history::{History, Turn};

/// The answer reserve for a call: a row with its arguments, `say` (240 bytes) or
/// `clarify` (six options). A script call carries no code (llm writes it with
/// the script prompt), so 256 is room to spare.
pub const ACT_MAX_TOKENS: u32 = 256;
/// Longest the llm service spends on one call.
pub const SERVICE_TIMEOUT_MS: u64 = 90_000;
pub const LINE_MAX_BYTES: usize = 512;
/// The State line's budget, 64 tokens (ADR 0010 §4), in bytes.
pub const STATE_MAX_BYTES: usize = 256;
/// The words of `script`'s `lang`. `c` stays out until ADR 0012's C fluency
/// gate passes.
pub const LANGS: &[&str] = &["rhai"];

/// What the shell asks the llm service for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActRequest {
    pub line: String,
    /// `focused=build-3 here=/home`, or "".
    pub state: String,
    pub timeout_ms: u64,
}

fn clip(text: &str, max: usize) -> &str {
    &text[..text.floor_char_boundary(max)]
}

impl ActRequest {
    /// `act <timeout> <line len> <state len>`, a newline, then the two texts.
    pub fn encode(&self) -> String {
        let line = clip(&self.line, LINE_MAX_BYTES);
        let state = clip(&self.state, STATE_MAX_BYTES);
        format!(
            "act {} {} {}\n{line}{state}",
            self.timeout_ms,
            line.len(),
            state.len()
        )
    }

    pub fn decode(text: &str) -> Option<Self> {
        let (header, body) = text.split_once('\n')?;
        let mut words = header.strip_prefix("act ")?.split(' ');
        let mut number = || words.next()?.parse::<usize>().ok();
        let (timeout, line_len, state_len) = (number()?, number()?, number()?);
        if body.len() != line_len.checked_add(state_len)?
            || line_len > LINE_MAX_BYTES
            || state_len > STATE_MAX_BYTES
        {
            return None;
        }
        Some(Self {
            line: body.get(..line_len)?.to_string(),
            state: body.get(line_len..)?.to_string(),
            timeout_ms: timeout as u64,
        })
    }
}

/// The current turn: the State line, if any, then the request.
pub fn user_text(state: &str, line: &str) -> String {
    match state {
        "" => format!("Request: {line}"),
        state => format!("State: {state}\nRequest: {line}"),
    }
}

/// What every `act` prompt starts with: the `dsl-1` pack, the `act` MODE turn,
/// then the history turns. The llm service warms the cache with it while idle.
pub fn prefix(history: &History) -> Vec<Message> {
    let mut messages = alloc::vec![
        Message::new(Role::System, &pack::render(Format::Dsl1)),
        Message::new(Role::System, MODE_ACT),
    ];
    messages.extend(history.messages());
    messages
}

/// The conversation: [`prefix`], then the request with its State line. Only
/// this last turn is the user's line; history is context, never consent.
pub fn request(act: &ActRequest, history: &History) -> chat::Request {
    let mut messages = prefix(history);
    messages.push(Message::new(Role::User, &user_text(&act.state, &act.line)));
    chat::Request {
        messages,
        max_tokens: ACT_MAX_TOKENS,
    }
}

/// A decoded call: a name and its arguments, as the model wrote them.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub name: String,
    pub arguments: Map<String, Value>,
}

impl Call {
    /// The call as the model wrote it, in the decoder's form.
    pub fn text(&self) -> String {
        serde_json::json!({"name": self.name, "arguments": self.arguments}).to_string()
    }
}

/// Why no call came back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActError {
    /// The answer stopped at the token limit before the call closed.
    CutOff,
    /// The text is not a call (a server's answer, or a broken wire).
    NotACall,
    /// The local model failed or ran out of time.
    Local(String),
    /// No model can answer: the text says what to do.
    Unavailable(String),
}

impl ActError {
    pub fn describe(&self) -> String {
        match self {
            Self::CutOff => "the call was cut off at the token limit".into(),
            Self::NotACall => "the answer was not a call".into(),
            Self::Local(why) => format!("local model: {why}"),
            Self::Unavailable(why) => why.clone(),
        }
    }
}

/// Reads a call: an object of exactly `name` (a string) and `arguments` (an
/// object). The decoder writes nothing else; this holds a wire to it.
pub fn parse(text: &str) -> Result<Call, ActError> {
    let value: Value = serde_json::from_str(text).map_err(|_| ActError::NotACall)?;
    let object = value
        .as_object()
        .filter(|o| o.len() == 2)
        .ok_or(ActError::NotACall)?;
    match (object.get("name"), object.get("arguments")) {
        (Some(Value::String(name)), Some(Value::Object(arguments))) => Ok(Call {
            name: name.clone(),
            arguments: arguments.clone(),
        }),
        _ => Err(ActError::NotACall),
    }
}

/// `ok <call>` or `err <why>` on the wire back to the shell.
pub fn encode_answer(answer: &Result<Call, ActError>) -> String {
    match answer {
        Ok(call) => format!("ok {}", call.text()),
        // The far end reads it back as `Local`, which adds "local model: ".
        Err(ActError::Local(why)) => format!("err {why}"),
        Err(error) => format!("err {}", error.describe()),
    }
}

pub fn decode_answer(text: &str) -> Result<Call, ActError> {
    match text.split_once(' ') {
        Some(("ok", call)) => parse(call),
        Some(("err", why)) => Err(ActError::Local(why.to_string())),
        _ => Err(ActError::NotACall),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_survive_the_wire_and_are_clipped_like_it() {
        let asked = ActRequest {
            line: "stop this".into(),
            state: "focused=build-3".into(),
            timeout_ms: 5000,
        };
        assert_eq!(ActRequest::decode(&asked.encode()), Some(asked));
        let long = ActRequest {
            line: "x".repeat(600),
            state: String::new(),
            timeout_ms: 1,
        };
        assert_eq!(
            ActRequest::decode(&long.encode()).unwrap().line.len(),
            LINE_MAX_BYTES
        );
        for bad in [
            "act 1 2 3\nab",
            "act x 0 0\n",
            "script 1 0 0\n",
            "act 1 600 0\n",
        ] {
            assert_eq!(ActRequest::decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_layout_is_pack_mode_then_state_and_request_with_no_think_tag() {
        let asked = request(
            &ActRequest {
                line: "stop this".into(),
                state: "focused=build-3".into(),
                timeout_ms: 1,
            },
            &History::new(),
        );
        assert_eq!(asked.messages.len(), 3);
        assert_eq!(asked.messages[0].content, pack::render(Format::Dsl1));
        assert_eq!(asked.messages[1].content, MODE_ACT);
        assert_eq!(
            asked.messages[2].content,
            "State: focused=build-3\nRequest: stop this"
        );
        assert!(!asked.messages[2].content.contains("/no_think"));
        // History goes between the MODE turn and the request.
        let mut history = History::new();
        let turn = Turn {
            line: "switch to round robin".into(),
            call: r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#.into(),
            result: "Done: policy round-robin (was interactive). undo".into(),
        };
        history.push(turn, 40, 512);
        let request = ActRequest {
            line: "undo that".into(),
            state: String::new(),
            timeout_ms: 1,
        };
        let asked = request_with(&request, &history);
        assert_eq!(asked.messages.len(), 6);
        assert_eq!(asked.messages[2].content, "Request: switch to round robin");
        assert_eq!(asked.messages[3].role, Role::Assistant);
        assert_eq!(asked.messages[5].content, "Request: undo that");
        assert_eq!(prefix(&history).len(), 5);
    }

    fn request_with(act: &ActRequest, history: &History) -> chat::Request {
        request(act, history)
    }

    #[test]
    fn only_a_call_parses_and_answers_survive_the_wire() {
        let call = parse(r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#).unwrap();
        assert_eq!(call.name, "set_policy");
        assert_eq!(decode_answer(&encode_answer(&Ok(call.clone()))), Ok(call));
        for bad in [
            "",
            "[]",
            r#"{"name":"x"}"#,
            r#"{"name":1,"arguments":{}}"#,
            r#"{"name":"x","arguments":[]}"#,
            r#"{"name":"x","arguments":{},"more":1}"#,
        ] {
            assert_eq!(parse(bad), Err(ActError::NotACall), "{bad}");
        }
        assert_eq!(
            decode_answer("err the call was cut off at the token limit"),
            Err(ActError::Local(
                "the call was cut off at the token limit".into()
            ))
        );
        // A local error crosses the wire once prefixed, not twice.
        let local = Err(ActError::Local("it ran out of time".into()));
        let back = decode_answer(&encode_answer(&local));
        assert_eq!(back, local);
        assert_eq!(
            back.unwrap_err().describe(),
            "local model: it ran out of time"
        );
    }
}
