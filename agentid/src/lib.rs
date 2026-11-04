//! Agents and identity (ADR 0013 section 1), as a host crate: the part of the
//! agent runtime that decides who is who.
//!
//! - [`roster`]: each agent's badge (the kernel's, minted at spawn and never reused),
//!   its name, and its `kaiagent` manifest. A packaged agent is installed from a
//!   checked manifest; its ceiling at the door and its budgets are the manifest's.
//! - [`door`]: [`Admission`], what `agentcore::execute` takes: the live caller and the
//!   row, checked against the agent's own grants (and again with `revalidate` when a
//!   waiting call runs), and the only source of consent words. An agent's call gets none, so every change it makes asks.
//! - [`mark`]: who made a key, a click or a line. The person's mark comes only from
//!   `unsafe { Source::hardware() }`, a claim by the input path; an agent's input cannot
//!   carry it.
//! - [`asks`]: a call's question, answered only by a key marked as the person's, and
//!   good once, for the agent and row that asked while it is still on the roster.
//! - [`stamp`]: the `agent=<name>` of every `actlog 2` entry, `undo <agent>`'s pick,
//!   and `kai` for a record that has none.
//!
//! The kernel delivers the badge and the compositor marks the input; this crate holds
//! the rules both feed. `no_std` + `alloc` and host-tested only: nothing in the OS uses
//! it yet, and it changes no input to the model.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod asks;
pub mod door;
pub mod mark;
pub mod name;
pub mod roster;
pub mod stamp;

#[cfg(test)]
mod tests;

pub use asks::{AnswerError, Answered, AskError, Asks};
pub use door::{Admission, Denied};
pub use mark::{Line, Mark, Source};
pub use name::{BadName, Name};
pub use roster::{
    Agent, AgentId, Builtin, Caller, InstallError, Kind, MAX_PACKAGED, Roster, UnknownBadge,
};
pub use stamp::{FieldError, Stamp, last_live, read_field};
