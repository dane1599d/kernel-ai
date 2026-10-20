//! The kernel's LLM wire protocol, kept free of networking so it can be tested on
//! the host: requests and answers in terms of no backend ([`chat`]), the prompts and
//! answer checks of `ask`, `explain` and `script`, HTTP/1.1 request building and
//! response parsing, and the OpenAI-compatible chat-completion JSON that
//! llama-server, Ollama and mlx_lm.server all speak.

#![no_std]

extern crate alloc;

pub mod chat;
pub mod explain;
pub mod http;
pub mod openai;
pub mod script;
pub mod text;
