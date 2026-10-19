//! Qwen3 inference on the CPU, without std: the model's shape from GGUF metadata,
//! every weight checked against it, and an f32 KV cache sized to a context cap.

#![no_std]

extern crate alloc;

mod config;
pub mod fixture;
mod kv;
mod weights;

#[cfg(test)]
mod tests;

pub use config::{Config, MAX_VOCAB};
pub use kv::{KvCache, Rope};
pub use weights::{Layer, Weights, check};

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
            Self::OutOfMemory => f.write_str("out of memory"),
        }
    }
}
