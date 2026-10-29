//! `explain`: the model explains why a live component's version was rolled back,
//! from facts the kernel takes from its own ledger. The answer is prose, shown only
//! under a line saying it is the model's, and nothing acts on it.

use crate::chat::{self, Completion};
use crate::openai::{CompletionError, ReplyError};
use crate::script::{SEED, TEMPERATURE};
use crate::text;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// What the model is asked to do with the facts.
pub const SYSTEM_PROMPT: &str = concat!(
    "You explain to the developer of ",
    kabi::product_name!(),
    ", a small \
experimental operating system, why a version of a live component was rolled back. Use \
only the facts you are given. Where they do not say why, say so; never guess a cause \
they do not support. Answer in two to four short sentences of plain text."
);
/// Most bytes of facts the kernel sends.
pub const MAX_FACTS: usize = 1024;
pub const MAX_TOKENS: u32 = 256;
/// The longest explanation shown.
pub const MAX_CHARS: usize = 600;
pub const MAX_LINES: usize = 8;
/// Error text from the server, cut to one console row.
const MAX_ERROR_CHARS: usize = 160;

/// The request for `facts`: the explainer's system prompt, then the facts with
/// thinking turned off.
pub fn request(facts: &str) -> chat::Request {
    let user = format!("Facts:\n{facts}\n/no_think");
    chat::Request::new(Some(SYSTEM_PROMPT), &user, MAX_TOKENS)
}

/// The chat request body for `facts`: thinking off, a fixed seed, plain text back.
pub fn chat_request(model: &str, facts: &str) -> Vec<u8> {
    let request = request(facts);
    let body = serde_json::json!({
        "model": model,
        "messages": request.messages,
        "max_tokens": request.max_tokens,
        "temperature": TEMPERATURE,
        "seed": SEED,
        "stream": false,
        "chat_template_kwargs": { "enable_thinking": false },
    });
    serde_json::to_vec(&body).unwrap_or_default()
}

/// The explanation in a completion, made printable and cut to [`MAX_CHARS`] and
/// [`MAX_LINES`]; marked when the model ran out of room.
pub fn answer(completion: &Completion) -> Result<String, String> {
    let content = text::without_thinking(&completion.text);
    let mut explanation = text::printable(content, MAX_CHARS, MAX_LINES);
    if explanation.trim().is_empty() {
        // A model that thinks anyway can spend every token before it answers.
        return Err(match completion.cut_off {
            true => String::from("the model ran out of tokens before answering; is thinking off?"),
            false => String::from("the model gave no explanation"),
        });
    }
    if completion.cut_off {
        explanation.push_str(chat::CUT_OFF);
    }
    Ok(explanation)
}

/// Why a server's response held no explanation, in one console row.
pub fn server_error(error: &CompletionError) -> String {
    match error {
        CompletionError::Status(status, body) => format!(
            "the server answered HTTP {status}: {}",
            text::printable(body, MAX_ERROR_CHARS, 1)
        ),
        CompletionError::Reply(ReplyError::Server(why)) => format!(
            "the server said: {}",
            text::printable(why, MAX_ERROR_CHARS, 1)
        ),
        CompletionError::Reply(other) => format!("bad chat reply: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{http, openai};
    use alloc::vec;

    /// A response whose content is `content`.
    fn response(content: &str, finish: &str) -> http::Response {
        let content = serde_json::to_string(content).unwrap();
        let body = format!(
            r#"{{"choices":[{{"message":{{"role":"assistant","content":{content}}},"finish_reason":"{finish}"}}]}}"#
        );
        http::Response {
            status: 200,
            body: body.into_bytes(),
        }
    }

    /// The explanation in an HTTP response, as the llm service reads one.
    fn answer_from(response: &http::Response) -> Result<String, String> {
        openai::completion(response)
            .map_err(|error| server_error(&error))
            .and_then(|completion| answer(&completion))
    }

    #[test]
    fn the_request_carries_the_facts_with_thinking_off_and_no_schema() {
        let facts = "component: notes\nlatest rollback: e2 (trap)";
        let request: serde_json::Value =
            serde_json::from_slice(&chat_request("qwen3-8b", facts)).unwrap();
        assert_eq!(request["model"], "qwen3-8b");
        assert_eq!(request["messages"][0]["role"], "system");
        assert_eq!(request["messages"][0]["content"], SYSTEM_PROMPT);
        assert_eq!(request["messages"][1]["role"], "user");
        assert_eq!(
            request["messages"][1]["content"],
            format!("Facts:\n{facts}\n/no_think")
        );
        assert_eq!(request["seed"], SEED);
        assert_eq!(request["max_tokens"], MAX_TOKENS);
        assert_eq!(request["chat_template_kwargs"]["enable_thinking"], false);
        assert!(request.get("response_format").is_none());
        assert_eq!(
            super::request(facts).single_turn(),
            Some((
                Some(SYSTEM_PROMPT),
                format!("Facts:\n{facts}\n/no_think").as_str()
            ))
        );
    }

    #[test]
    fn an_answer_is_cleaned_and_cut() {
        assert_eq!(
            answer_from(&response("It trapped.", "stop")),
            Ok(String::from("It trapped."))
        );
        assert_eq!(
            answer_from(&response("<think>hmm</think>\n It trapped.", "stop")),
            Ok(String::from("It trapped."))
        );
        assert_eq!(
            answer_from(&response("It\x1b[2J trapped.\r", "stop")),
            Ok(String::from("It[2J trapped."))
        );
        assert_eq!(
            answer_from(&response("It trapped", "length")),
            Ok(String::from("It trapped (cut off)"))
        );
        let long = "word ".repeat(400);
        let cut = answer_from(&response(&long, "stop")).unwrap();
        assert_eq!(cut.chars().count(), MAX_CHARS);
        let many = "line\n".repeat(20);
        assert_eq!(
            answer_from(&response(&many, "stop"))
                .unwrap()
                .lines()
                .count(),
            MAX_LINES
        );
    }

    #[test]
    fn no_explanation_or_a_failing_server_is_an_error() {
        for content in ["", "  ", "<think>never ends"] {
            assert_eq!(
                answer_from(&response(content, "stop")),
                Err(String::from("the model gave no explanation")),
                "{content:?}"
            );
            assert_eq!(
                answer_from(&response(content, "length")),
                Err(String::from(
                    "the model ran out of tokens before answering; is thinking off?"
                )),
                "{content:?}"
            );
        }
        let thought_only = http::Response {
            status: 200,
            body: br#"{"choices":[{"message":{"content":null,"reasoning_content":"hmm"},"finish_reason":"length"}]}"#.to_vec(),
        };
        assert!(
            answer_from(&thought_only)
                .unwrap_err()
                .starts_with("the model ran out")
        );
        let refused = http::Response {
            status: 500,
            body: b"overloaded\nretry".to_vec(),
        };
        assert_eq!(
            answer_from(&refused),
            Err(String::from("the server answered HTTP 500: overloaded"))
        );
        let failed = http::Response {
            status: 200,
            body: br#"{"error":{"message":"no model"}}"#.to_vec(),
        };
        assert_eq!(
            answer_from(&failed),
            Err(String::from("the server said: no model"))
        );
        let garbage = http::Response {
            status: 200,
            body: vec![b'x'],
        };
        assert!(
            answer_from(&garbage)
                .unwrap_err()
                .starts_with("bad chat reply")
        );
    }

    #[test]
    fn a_completion_from_any_backend_reads_the_same() {
        let local = Completion {
            text: String::from(" The candidate trapped in count.\n"),
            cut_off: false,
        };
        assert_eq!(
            answer(&local),
            Ok(String::from("The candidate trapped in count."))
        );
    }
}
