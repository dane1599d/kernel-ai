//! OpenAI-compatible chat completions over HTTP, non-streaming: the body for a
//! [`chat::Request`] and the [`chat::Completion`] in a server's response.

use crate::chat::{self, Completion, Message};
use crate::http;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

pub const CHAT_PATH: &str = "/v1/chat/completions";

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    max_tokens: u32,
    temperature: f32,
    stream: bool,
}

/// Serializes `request` for `model`, at temperature 0.2, as `ask` sends it.
pub fn chat_request(model: &str, request: &chat::Request) -> Vec<u8> {
    let body = ChatRequest {
        model,
        messages: &request.messages,
        max_tokens: request.max_tokens,
        temperature: 0.2,
        stream: false,
    };
    serde_json::to_vec(&body).unwrap_or_default()
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct Choice {
    message: ReplyMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ReplyMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct ApiError {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyError {
    NotJson,
    /// The server answered with an error object.
    Server(String),
    NoChoices,
}

/// Why a response holds no completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionError {
    /// The server answered with a status other than 200; its body, as text (empty
    /// when it is not UTF-8).
    Status(u16, String),
    /// A 200 whose body is not a chat completion.
    Reply(ReplyError),
}

/// The deepest a reply's arrays and objects may nest. Real replies nest six deep
/// at most (`choices[].logprobs.content[].top_logprobs[]`); serde_json would
/// otherwise skip unknown fields 128 levels deep, recursing on the llm service's
/// 64 KiB stack.
pub const MAX_JSON_DEPTH: usize = 32;

/// Whether no array or object in `json` nests deeper than `max`. Brackets inside
/// strings do not count; anything that is not JSON is left to the parser.
fn nesting_within(json: &[u8], max: usize) -> bool {
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    for &b in json {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                if depth > max {
                    return false;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    true
}

/// The completion in a chat-completion response body. A missing or empty content
/// is an empty text; `cut_off` is a `length` finish. A body nested deeper than
/// [`MAX_JSON_DEPTH`] is not JSON this reads.
pub fn parse_completion(body: &[u8]) -> Result<Completion, ReplyError> {
    if !nesting_within(body, MAX_JSON_DEPTH) {
        return Err(ReplyError::NotJson);
    }
    let response: ChatResponse = serde_json::from_slice(body).map_err(|_| ReplyError::NotJson)?;
    if let Some(error) = response.error {
        return Err(ReplyError::Server(error.message));
    }
    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or(ReplyError::NoChoices)?;
    Ok(Completion {
        text: choice.message.content.unwrap_or_default(),
        cut_off: choice.finish_reason.as_deref() == Some("length"),
    })
}

/// The completion in a whole HTTP response: the status first, then the body.
pub fn completion(response: &http::Response) -> Result<Completion, CompletionError> {
    if response.status != 200 {
        let body = core::str::from_utf8(&response.body).unwrap_or("");
        return Err(CompletionError::Status(response.status, String::from(body)));
    }
    parse_completion(&response.body).map_err(CompletionError::Reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(body: &[u8]) -> http::Response {
        http::Response {
            status: 200,
            body: body.to_vec(),
        }
    }

    #[test]
    fn request_has_the_fields_servers_expect() {
        let request = chat::Request::new(Some("be brief"), "hi \"there\"", 64);
        let body = chat_request("qwen3-8b", &request);
        let text = core::str::from_utf8(&body).unwrap();
        assert_eq!(
            text,
            r#"{"model":"qwen3-8b","messages":[{"role":"system","content":"be brief"},{"role":"user","content":"hi \"there\""}],"max_tokens":64,"temperature":0.2,"stream":false}"#
        );
    }

    #[test]
    fn parses_reply_text_including_escapes_and_unicode() {
        // JSON escapes built at run time: a backslash, then u00e9 (e acute) and a
        // surrogate pair for U+1F600, then n for a newline.
        let body = alloc::format!(
            r#"{{"id":"x","choices":[{{"index":0,"message":{{"role":"assistant","content":"Hi {b}u00e9 {b}ud83d{b}ude00{b}nok"}},"finish_reason":"stop"}}]}}"#,
            b = char::from(92u8)
        );
        let reply = completion(&ok(body.as_bytes())).unwrap();
        assert_eq!(reply.text, "Hi \u{e9} \u{1F600}\nok");
        assert!(!reply.cut_off);
    }

    #[test]
    fn a_length_finish_is_cut_off_and_no_content_is_empty_text() {
        let cut = completion(&ok(
            br#"{"choices":[{"message":{"content":"Hi"},"finish_reason":"length"}]}"#,
        ))
        .unwrap();
        assert_eq!(
            cut,
            Completion {
                text: String::from("Hi"),
                cut_off: true
            }
        );
        let thought_only = completion(&ok(
            br#"{"choices":[{"message":{"content":null,"reasoning_content":"hmm"},"finish_reason":"length"}]}"#,
        ))
        .unwrap();
        assert_eq!(thought_only.text, "");
        assert!(thought_only.cut_off);
        let empty = completion(&ok(
            br#"{"choices":[{"message":{"role":"assistant","content":""}}]}"#,
        ))
        .unwrap();
        assert_eq!(empty, Completion::default());
    }

    #[test]
    fn reports_server_errors_and_bad_bodies() {
        assert_eq!(
            completion(&ok(
                br#"{"error":{"message":"model not found","type":"x"}}"#
            )),
            Err(CompletionError::Reply(ReplyError::Server(
                "model not found".into()
            )))
        );
        assert_eq!(
            completion(&ok(br#"{"choices":[]}"#)),
            Err(CompletionError::Reply(ReplyError::NoChoices))
        );
        assert_eq!(
            completion(&ok(b"<html>")),
            Err(CompletionError::Reply(ReplyError::NotJson))
        );
        let refused = http::Response {
            status: 503,
            body: b"busy".to_vec(),
        };
        assert_eq!(
            completion(&refused),
            Err(CompletionError::Status(503, String::from("busy")))
        );
    }
}
