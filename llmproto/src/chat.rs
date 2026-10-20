//! Chat requests and answers that do not depend on how the model is reached: the
//! messages of a request, and what a model gave back, whether it came from an
//! OpenAI-compatible server over HTTP ([`crate::openai`]) or from the engine inside
//! the llm service. The answer checks in [`crate::explain`] and [`crate::script`]
//! read a [`Completion`], so every backend shares them.

use alloc::string::String;
use alloc::vec::Vec;
use serde::Serialize;

/// The system prompt of `ask`.
pub const ASK_SYSTEM_PROMPT: &str = "You are the assistant built into Kernel AI, a small \
                                     experimental operating system. Answer in a few short \
                                     sentences.";
pub const ASK_MAX_TOKENS: u32 = 256;
/// The longest answer to `ask` shown.
pub const MAX_REPLY_CHARS: usize = 4096;
pub const MAX_REPLY_LINES: usize = 64;
/// Added to an answer the model did not end itself.
pub const CUT_OFF: &str = " (cut off)";

/// Who wrote a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// One message of a conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn new(role: Role, content: &str) -> Self {
        Self {
            role,
            content: String::from(content),
        }
    }
}

/// What to ask a model: the conversation so far, and the most tokens it may answer
/// with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub messages: Vec<Message>,
    pub max_tokens: u32,
}

impl Request {
    /// An optional system prompt and one user turn.
    pub fn new(system: Option<&str>, user: &str, max_tokens: u32) -> Self {
        let mut messages = Vec::new();
        if let Some(system) = system {
            messages.push(Message::new(Role::System, system));
        }
        messages.push(Message::new(Role::User, user));
        Self {
            messages,
            max_tokens,
        }
    }

    /// The system prompt and the user's text, when the request is exactly an
    /// optional system message and then one user message: what a backend that
    /// renders a single turn takes.
    pub fn single_turn(&self) -> Option<(Option<&str>, &str)> {
        match self.messages.as_slice() {
            [user] if user.role == Role::User => Some((None, &user.content)),
            [system, user] if system.role == Role::System && user.role == Role::User => {
                Some((Some(&system.content), &user.content))
            }
            _ => None,
        }
    }
}

/// What a model gave back for a request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Completion {
    /// The answer as the model wrote it, which may be empty or hold anything.
    pub text: String,
    /// The model did not end the answer itself: it reached its token limit, the
    /// end of its context or a time limit.
    pub cut_off: bool,
}

/// The `ask` request for `question`.
pub fn ask(question: &str) -> Request {
    Request::new(Some(ASK_SYSTEM_PROMPT), question, ASK_MAX_TOKENS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_turn_is_read_back_and_anything_else_is_not() {
        let asked = ask("What is a kernel?");
        assert_eq!(
            asked.single_turn(),
            Some((Some(ASK_SYSTEM_PROMPT), "What is a kernel?"))
        );
        assert_eq!(asked.max_tokens, ASK_MAX_TOKENS);
        let bare = Request::new(None, "hi", 8);
        assert_eq!(bare.single_turn(), Some((None, "hi")));
        let mut repaired = Request::new(Some("s"), "u", 8);
        repaired.messages.push(Message::new(Role::Assistant, "a"));
        repaired.messages.push(Message::new(Role::User, "again"));
        assert_eq!(repaired.single_turn(), None);
        let backwards = Request {
            messages: alloc::vec![
                Message::new(Role::User, "u"),
                Message::new(Role::System, "s")
            ],
            max_tokens: 8,
        };
        assert_eq!(backwards.single_turn(), None);
    }

    #[test]
    fn messages_serialize_as_openai_expects() {
        let message = Message::new(Role::Assistant, "hi \"there\"");
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"role":"assistant","content":"hi \"there\""}"#
        );
    }
}
