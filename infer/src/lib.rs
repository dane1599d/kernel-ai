//! Qwen3 inference on the CPU, without std: the model's shape from GGUF metadata,
//! every weight checked against it, an f32 KV cache sized to a context cap, the
//! forward pass over the Q8_0 weights in the file, greedy generation, seeded
//! sampling ([`Sampler`]), decoding constrained to a JSON shape ([`json`]), and
//! [`chat`], one answer to a conversation the way the llm service runs it.

#![no_std]

extern crate alloc;

mod acts;
mod attend;
pub mod chat;
mod config;
pub mod fixture;
mod generate;
pub mod json;
mod kv;
mod model;
mod sample;
pub mod segments;
pub mod tagged;
mod weights;

#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod chat_tests;
#[cfg(test)]
mod forward_tests;
#[cfg(test)]
mod json_tests;
#[cfg(test)]
mod kquant_tests;
#[cfg(test)]
mod prefill_tests;
#[cfg(test)]
mod reference;
#[cfg(test)]
mod reply_tests;
#[cfg(test)]
mod segments_tests;
#[cfg(test)]
mod tests;

pub use config::{Config, MAX_VOCAB};
pub use generate::{Ended, Next, argmax, generate};
pub use kv::{KvCache, Rope};
pub use model::{Model, PREFILL_TOKENS};
pub use sample::Sampler;
pub use weights::{Layer, Matrix, Weights, check};

use alloc::string::String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// `general.architecture` is not `qwen3`.
    Architecture,
    /// The model needs something the engine does not do.
    Unsupported(&'static str),
    MissingKey(&'static str),
    /// A metadata value is the wrong type or out of range.
    BadValue(&'static str),
    /// A tensor is missing, misshapen, mistyped or unknown.
    Tensor(String, &'static str),
    /// Two tensors share bytes of the file.
    Overlap,
    /// A metadata key the engine does not know, which might change what the
    /// model means.
    UnknownKey(String),
    /// A context cap of zero or past what the model was trained for.
    Context,
    /// Every position of the KV cache is used.
    ContextFull,
    /// A KV cache made for another model shape or a longer context.
    CacheShape,
    /// A prompt with no tokens.
    EmptyPrompt,
    /// A logits buffer that is not one value per vocabulary entry.
    Logits,
    /// A token id past the vocabulary.
    Token(u32),
    /// A JSON shape that cannot be decoded, or a vocabulary that cannot spell it.
    Shape(&'static str),
    /// The token limit or the context leaves no room to close a JSON shape.
    NoRoom,
    OutOfMemory,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Architecture => f.write_str("not a qwen3 model"),
            Self::Unsupported(what) => write!(f, "the engine does not support {what}"),
            Self::MissingKey(key) => write!(f, "the file has no {key}"),
            Self::BadValue(key) => write!(f, "{key} is out of range"),
            Self::Tensor(name, problem) => write!(f, "tensor {name} {problem}"),
            Self::Overlap => f.write_str("two tensors share bytes of the file"),
            Self::UnknownKey(key) => {
                write!(f, "unknown metadata {key}, which the engine cannot honour")
            }
            Self::Context => f.write_str("the context cap is zero or too long for the model"),
            Self::ContextFull => f.write_str("the context is full"),
            Self::CacheShape => f.write_str("the KV cache was made for another model"),
            Self::EmptyPrompt => f.write_str("the prompt is empty"),
            Self::Logits => f.write_str("the logits buffer does not match the vocabulary"),
            Self::Token(id) => write!(f, "token {id} is past the vocabulary"),
            Self::Shape(problem) => write!(f, "the JSON shape {problem}"),
            Self::NoRoom => {
                f.write_str("the token limit or the context leaves no room for the JSON answer")
            }
            Self::OutOfMemory => f.write_str("out of memory"),
        }
    }
}
