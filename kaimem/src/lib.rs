//! Kernel AI's long-term memory (ADR 0013 §3): facts and preferences the user
//! lets it keep.
//!
//! - [`record`]: one memory, and `kai-memory 1`, the file that holds them;
//! - [`store`]: the memories, with edit, and forget that can be undone for
//!   10 minutes and then purges;
//! - [`remember`]: who may make one. The user's own "remember …" line runs; a
//!   memory the model proposes asks with its exact text; one from Content is
//!   refused, so a file, a note or a page cannot plant one;
//! - [`directive`]: a text that permits or sets a standing action is no
//!   memory, whoever asks;
//! - [`prompt`]: the memory section of a request's prompt, at most 8 memories,
//!   marked as the user's earlier words. They inform the model and never give
//!   consent: consent comes only from the current line.
//!
//! Host-tested only: the `memory` volume, the rows and the shell's wiring come
//! after the v0.1 tag.

#![no_std]

extern crate alloc;

pub mod directive;
pub mod prompt;
pub mod record;
pub mod remember;
pub mod store;

#[cfg(test)]
mod tests;
