//! The kernel's LLM wire protocol, kept free of networking so it can be tested on
//! the host: HTTP/1.1 request building and response parsing, and the
//! OpenAI-compatible chat-completion JSON that llama-server, Ollama and
//! mlx_lm.server all speak.

#![no_std]

extern crate alloc;

pub mod explain;
pub mod http;
pub mod openai;
pub mod script;
pub mod text;
