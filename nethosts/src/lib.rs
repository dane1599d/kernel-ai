//! Hosts and grounding for the agent's network rows (P5 task 11's host half; ADR
//! 0002 D3): the code that says whether a fetch runs or asks. The rows themselves
//! (what the model sees) are the agent core's to add, with a fluency run; this
//! crate is the table and the rule they will use, with no OS in it.
//!
//! - [`Host`]: one normal form, so `WWW.YouTube.com.` is `www.youtube.com` and
//!   `www.youtube.com.evil.example` or `www.youtube.com@evil.example` is not.
//!   [`typed_hosts`] finds the hosts the user's line names.
//! - [`Table`]: the curated host table, a request word and the host it means
//!   ("youtube", and an installed app's own word, "weather"). No app can shadow a
//!   shipped word or share another app's.
//! - [`Grounding::decide`]: the rule. A fetch runs from a host the user typed, one
//!   a word of the line means, one the user said yes to before, or one on the OS's
//!   own list whose bytes are checked against signed metadata; any other asks.
//!   Read content never grounds a host. [`Ground::names_destination`] is what P-F
//!   means by a destination the line names.
//! - [`texts`]: the ask, the note on why a fetch ran, and the result line.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod ground;
mod host;
mod table;
pub mod texts;

#[cfg(test)]
mod tests;

pub use ground::{
    Bytes, Granted, Grants, Ground, Grounding, NoGrants, Origin, Source, Ungrounded, Verdict,
};
pub use host::{Host, HostError, MAX_HOST, MAX_TYPED, typed_hosts};
pub use table::{
    CURATED, Entry, MAX_APP_HOSTS, MAX_ENTRIES, MAX_WORD, MIN_WORD, Provider, Table, TableError,
};
