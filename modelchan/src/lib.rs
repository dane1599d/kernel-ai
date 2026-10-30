//! The model channel (ADR 0008 §6; P4 task 10): what `updated` decides about a
//! model, apart from the handles it decides with.
//!
//! - [`import`]: a model read from an attached disk, hashed as it streams and
//!   held to the signed `models` role before it is kept (`comp_models::check`);
//! - [`trial`]: `kai-modeltrial 1`, the record of a model chosen for the next
//!   restart, and the probation that keeps it or goes back to the model before.
//!
//! Both are pure: `updated` reads the disk, writes the store through `fs`, and
//! asks `llm` for its health, and hands what it got to these.

#![no_std]

extern crate alloc;

pub mod import;
pub mod trial;

pub use comp_models::Sha256;
