//! OpenAI-compatible chat completions, non-streaming.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

pub const CHAT_PATH: &str = "/v1/chat/completions";

/// One message of a conversation.
#[derive(Serialize, Clone, Copy, Debug)]
pub struct ChatMessage<'a> {
    pub role: &'a str,
    pub content: &'a str,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    max_tokens: u32,
    temperature: f32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<serde_json::Value>,
}

/// Serializes a chat request with an optional system prompt and one user turn.
pub fn chat_request(model: &str, system: Option<&str>, user: &str, max_tokens: u32) -> Vec<u8> {
    let mut messages = Vec::new();
    if let Some(system) = system {
        messages.push(ChatMessage {
            role: "system",
            content: system,
        });
    }
    messages.push(ChatMessage {
        role: "user",
        content: user,
    });
    let request = ChatRequest {
        model,
        messages,
        max_tokens,
        temperature: 0.2,
        stream: false,
        response_format: None,
    };
    serde_json::to_vec(&request).unwrap_or_default()
}

/// Serializes a chat request for a whole conversation. `response_format`, if given,
/// is sent as is (for example a `json_schema` object, which llama-server, Ollama
/// and LM Studio all accept).
pub fn chat_request_messages(
    model: &str,
    messages: &[ChatMessage<'_>],
    max_tokens: u32,
    temperature: f32,
    response_format: Option<serde_json::Value>,
) -> Vec<u8> {
    let request = ChatRequest {
        model,
        messages: messages.to_vec(),
        max_tokens,
        temperature,
        stream: false,
        response_format,
    };
    serde_json::to_vec(&request).unwrap_or_default()
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
    EmptyContent,
}

/// The assistant's answer and why the model stopped (`stop`, `length`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatReply {
    pub content: String,
    pub finish_reason: Option<String>,
}

/// Extracts the assistant's answer and the stop reason from a response body.
pub fn parse_chat_reply(body: &[u8]) -> Result<ChatReply, ReplyError> {
    let response: ChatResponse = serde_json::from_slice(body).map_err(|_| ReplyError::NotJson)?;
    if let Some(error) = response.error {
        return Err(ReplyError::Server(error.message));
    }
    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or(ReplyError::NoChoices)?;
    let content = choice
        .message
        .content
        .filter(|content| !content.is_empty())
        .ok_or(ReplyError::EmptyContent)?;
    Ok(ChatReply {
        content,
        finish_reason: choice.finish_reason,
    })
}

/// Extracts the assistant's text from a chat-completion response body.
pub fn parse_chat_response(body: &[u8]) -> Result<String, ReplyError> {
    parse_chat_reply(body).map(|reply| reply.content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_has_the_fields_servers_expect() {
        let body = chat_request("qwen3-8b", Some("be brief"), "hi \"there\"", 64);
        let text = core::str::from_utf8(&body).unwrap();
        assert!(text.contains(r#""model":"qwen3-8b""#));
        assert!(text.contains(r#"{"role":"system","content":"be brief"}"#));
        assert!(text.contains(r#"{"role":"user","content":"hi \"there\""}"#));
        assert!(text.contains(r#""max_tokens":64"#));
        assert!(text.contains(r#""stream":false"#));
    }

    #[test]
    fn parses_reply_text_including_escapes_and_unicode() {
        // JSON escapes built at run time: a backslash, then u00e9 (e acute) and a
        // surrogate pair for U+1F600, then n for a newline.
        let body = alloc::format!(
            r#"{{"id":"x","choices":[{{"index":0,"message":{{"role":"assistant","content":"Hi {b}u00e9 {b}ud83d{b}ude00{b}nok"}},"finish_reason":"stop"}}]}}"#,
            b = char::from(92u8)
        );
        let body = body.as_bytes();
        assert_eq!(
            parse_chat_response(body).unwrap(),
            "Hi \u{e9} \u{1F600}\nok"
        );
    }

    #[test]
    fn reports_server_errors_and_bad_bodies() {
        assert_eq!(
            parse_chat_response(br#"{"error":{"message":"model not found","type":"x"}}"#),
            Err(ReplyError::Server("model not found".into()))
        );
        assert_eq!(
            parse_chat_response(br#"{"choices":[]}"#),
            Err(ReplyError::NoChoices)
        );
        assert_eq!(
            parse_chat_response(br#"{"choices":[{"message":{"role":"assistant","content":""}}]}"#),
            Err(ReplyError::EmptyContent)
        );
        assert_eq!(parse_chat_response(b"<html>"), Err(ReplyError::NotJson));
    }
}
