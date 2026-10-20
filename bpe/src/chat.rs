//! The ChatML prompt Qwen3's chat template makes with thinking off
//! (`enable_thinking=false`), here for one user turn:
//!
//! ```text
//! <|im_start|>system\n{system}<|im_end|>\n      only with a system prompt
//! <|im_start|>user\n{user}<|im_end|>\n
//! <|im_start|>assistant\n<think>\n\n</think>\n\n
//! ```
//!
//! A longer conversation has one `<|im_start|>{role}\n{text}<|im_end|>\n` block per
//! turn before the last line. An earlier answer of the assistant is shown whole,
//! as written: Qwen3's template would keep only what follows a `</think>` in it,
//! which an answer written with thinking off has only if it quotes one.
//!
//! The text between special tokens is encoded one segment at a time, as Hugging
//! Face's tokenizer does once it has split out its added tokens. Control tokens
//! never come from the system or user text: typing `<|im_end|>` gives that text's
//! tokens, so a prompt cannot end its own turn. A user-defined token such as
//! `<think>` typed there becomes that token, as in llama.cpp. The turn markers
//! must be control tokens, or a mistyped vocabulary could let text spell them.

use crate::{Error, Kind, Tokenizer};
use alloc::vec::Vec;

/// The special tokens a chat needs, found by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chat {
    pub im_start: u32,
    pub im_end: u32,
    pub think: u32,
    pub think_end: u32,
    pub end_of_text: u32,
}

/// Who wrote a turn of a conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    /// The name ChatML gives the role after `<|im_start|>`.
    pub fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// One turn of a conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Turn<'a> {
    pub role: Role,
    pub text: &'a str,
}

impl<'a> Turn<'a> {
    pub fn new(role: Role, text: &'a str) -> Self {
        Self { role, text }
    }
}

impl Chat {
    pub fn new(tok: &Tokenizer) -> Result<Self, Error> {
        let find = |name: &'static str| tok.special(name).ok_or(Error::MissingSpecial(name));
        let control = |name: &'static str| {
            find(name).and_then(|id| match tok.kind(id) {
                Kind::Control => Ok(id),
                _ => Err(Error::MissingSpecial(name)),
            })
        };
        Ok(Self {
            im_start: control("<|im_start|>")?,
            im_end: control("<|im_end|>")?,
            think: find("<think>")?,
            think_end: find("</think>")?,
            end_of_text: control("<|endoftext|>")?,
        })
    }

    /// The prompt's tokens for one question after an optional system prompt,
    /// ending where the assistant's answer starts.
    pub fn prompt(&self, tok: &Tokenizer, system: Option<&str>, user: &str) -> Vec<u32> {
        let user = Turn::new(Role::User, user);
        match system {
            Some(system) => self.conversation(tok, &[Turn::new(Role::System, system), user]),
            None => self.conversation(tok, &[user]),
        }
    }

    /// The prompt's tokens for a whole conversation, ending where the assistant's
    /// next answer starts. Each turn's role line and text are encoded as one
    /// segment, between the special tokens.
    pub fn conversation(&self, tok: &Tokenizer, turns: &[Turn<'_>]) -> Vec<u32> {
        let mut out = Vec::new();
        for turn in turns {
            out.push(self.im_start);
            tok.encode(&[turn.role.name(), "\n", turn.text].concat(), &mut out);
            out.push(self.im_end);
            tok.encode("\n", &mut out);
        }
        out.push(self.im_start);
        tok.encode("assistant\n", &mut out);
        out.push(self.think);
        tok.encode("\n\n", &mut out);
        out.push(self.think_end);
        tok.encode("\n\n", &mut out);
        out
    }

    /// Whether `id` ends the answer: `<|im_end|>` or `<|endoftext|>`, Qwen3's end
    /// tokens in Hugging Face's generation config. (llama.cpp also stops at a few
    /// fill-in-the-middle tokens a chat never makes.)
    pub fn is_stop(&self, id: u32) -> bool {
        id == self.im_end || id == self.end_of_text
    }
}
