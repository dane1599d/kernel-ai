//! The ChatML prompt Qwen3's chat template makes for one user turn with thinking
//! off (`enable_thinking=false`):
//!
//! ```text
//! <|im_start|>system\n{system}<|im_end|>\n      only with a system prompt
//! <|im_start|>user\n{user}<|im_end|>\n
//! <|im_start|>assistant\n<think>\n\n</think>\n\n
//! ```
//!
//! The text between special tokens is encoded one segment at a time, as Hugging
//! Face's tokenizer does once it has split out its added tokens. Control tokens
//! never come from the system or user text: typing `<|im_end|>` gives that text's
//! tokens, so a prompt cannot end its own turn. A user-defined token such as
//! `<think>` typed there becomes that token, as in llama.cpp. The turn markers
//! must be control tokens, or a mistyped vocabulary could let text spell them.

use crate::{Error, Kind, Tokenizer};
use alloc::string::String;
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

enum Part<'a> {
    Special(u32),
    Text(&'a str),
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

    /// The prompt's tokens, ending where the assistant's answer starts.
    pub fn prompt(&self, tok: &Tokenizer, system: Option<&str>, user: &str) -> Vec<u32> {
        let system = system.map(|text| ["system\n", text].concat());
        let user: String = ["user\n", user].concat();
        let mut parts = Vec::new();
        if let Some(system) = &system {
            parts.extend([
                Part::Special(self.im_start),
                Part::Text(system),
                Part::Special(self.im_end),
                Part::Text("\n"),
            ]);
        }
        parts.extend([
            Part::Special(self.im_start),
            Part::Text(&user),
            Part::Special(self.im_end),
            Part::Text("\n"),
            Part::Special(self.im_start),
            Part::Text("assistant\n"),
            Part::Special(self.think),
            Part::Text("\n\n"),
            Part::Special(self.think_end),
            Part::Text("\n\n"),
        ]);
        let mut out = Vec::new();
        for part in parts {
            match part {
                Part::Special(id) => out.push(id),
                Part::Text(text) => tok.encode(text, &mut out),
            }
        }
        out
    }

    /// Whether `id` ends the answer: `<|im_end|>` or `<|endoftext|>`, Qwen3's end
    /// tokens in Hugging Face's generation config. (llama.cpp also stops at a few
    /// fill-in-the-middle tokens a chat never makes.)
    pub fn is_stop(&self, id: u32) -> bool {
        id == self.im_end || id == self.end_of_text
    }
}
