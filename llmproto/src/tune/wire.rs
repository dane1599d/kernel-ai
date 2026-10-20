//! `tune` on the llm service's wire: the shell sends the summary's text and the
//! names the table lets the model label; the service sends back the model's
//! completion as it came, and the shell runs [`super::decide`] on it. The llm
//! service is never trusted for validity, only for the words.
//!
//! Request: `tune <name>,<name>,...` then a newline and the summary text. Reply
//! (after the service's `ok `): `=` and the completion, or `!` and the
//! completion when it was cut off.

use super::{ANSWER_TOKENS, NO_THINK, response_format, system_prompt};
use crate::chat::{self, Completion};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// What the shell asks: the summary text and the labelable names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuneRequest {
    pub labelable: Vec<String>,
    pub text: String,
}

/// A name the wire can carry, as the summary makes them: `[a-z0-9-]` only.
fn plain(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl TuneRequest {
    pub fn encode(&self) -> String {
        let names: Vec<&str> = self
            .labelable
            .iter()
            .map(String::as_str)
            .filter(|name| plain(name))
            .collect();
        format!("tune {}\n{}", names.join(","), self.text)
    }

    /// The request, or `None` if it is not one or names something no summary
    /// would.
    pub fn decode(wire: &str) -> Option<Self> {
        let rest = wire.strip_prefix("tune ")?;
        let (names, text) = rest.split_once('\n')?;
        let labelable: Vec<String> = match names {
            "" => Vec::new(),
            names => names.split(',').map(ToString::to_string).collect(),
        };
        if !labelable.iter().all(|name| plain(name)) || text.trim().is_empty() {
            return None;
        }
        Some(Self {
            labelable,
            text: text.to_string(),
        })
    }

    /// The conversation every backend gets: the policy cards as the system
    /// prompt (a fixed prefix, which a local engine keeps cached), the summary,
    /// and thinking off.
    pub fn chat(&self) -> chat::Request {
        let user = format!("{}\n{NO_THINK}", self.text);
        chat::Request::new(Some(system_prompt()), &user, ANSWER_TOKENS)
    }

    /// The OpenAI-compatible body for a server: [`Self::chat`] with
    /// `response_format` and greedy decoding.
    pub fn chat_body(&self, model: &str) -> Vec<u8> {
        let asked = self.chat();
        let format = serde_json::from_str::<serde_json::Value>(&response_format(&self.labelable))
            .unwrap_or(serde_json::Value::Null);
        let body = serde_json::json!({
            "model": model,
            "messages": asked.messages,
            "max_tokens": asked.max_tokens,
            "temperature": 0,
            "stream": false,
            "response_format": format,
            "chat_template_kwargs": { "enable_thinking": false },
        });
        serde_json::to_vec(&body).unwrap_or_default()
    }
}

/// The completion as the service sends it back.
pub fn encode_completion(completion: &Completion) -> String {
    let mark = if completion.cut_off { '!' } else { '=' };
    format!("{mark}{}", completion.text)
}

pub fn decode_completion(wire: &str) -> Option<Completion> {
    let cut_off = match wire.as_bytes().first()? {
        b'=' => false,
        b'!' => true,
        _ => return None,
    };
    Some(Completion {
        text: wire[1..].to_string(),
        cut_off,
    })
}
