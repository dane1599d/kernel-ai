//! The `c>` prompt's front end (ADR 0012 D1-D4, D11), shared by `cc` and the
//! shell:
//!
//! - [`lex::open`]: whether an entry needs another line (`more`, `..> `);
//! - [`Defs::prepare`]: an entry as a compile unit, with the five conveniences
//!   and the prelude of live definitions ([`prepare`]);
//! - [`Defs`]: the live definitions and the undo journal ([`defs`]);
//! - [`wire`]: `kai-cc 1`, the messages between the shell and `cc`.
//!
//! Entries and messages are untrusted text: every parser here returns an error
//! with a line rather than panicking, and its work is bounded by its input
//! (`tests/hostile.rs`). Nothing here compiles, links or runs code; `cc` does,
//! inside wasmi.

#![no_std]

extern crate alloc;

pub mod defs;
pub mod lex;
pub mod prepare;
pub mod split;
pub mod wire;

pub use defs::{Def, Defs, Kind, Undone};
pub use lex::open;
pub use prepare::{LineMap, Mode, Prepared};

use core::fmt;

/// Longest entry, in bytes (D1).
pub const MAX_ENTRY: usize = 4096;
/// kai.h's macro that shows a value by its type (convenience 2).
pub const SHOW: &str = "__kai_show";
/// kai.h's formatted print (conveniences 3 and 5).
pub const PRINT: &str = "Print";
/// The run function of entry N is `__kai_run_N` (convenience 1).
pub const RUN_PREFIX: &str = "__kai_run_";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Over [`MAX_ENTRY`].
    TooLong,
    Lex(lex::LexError),
    /// The entry's shape, at a line of it.
    Syntax {
        line: u32,
        what: &'static str,
    },
    /// The session holds [`defs::MAX_DEFS`] definitions.
    Full,
}

impl Error {
    /// The entry line it is about; 0 when none.
    pub fn line(&self) -> u32 {
        match self {
            Error::Lex(e) => e.line(),
            Error::Syntax { line, .. } => *line,
            Error::TooLong | Error::Full => 0,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooLong => write!(f, "an entry is at most {MAX_ENTRY} bytes"),
            Error::Lex(e) => write!(f, "{e}"),
            Error::Syntax { what, .. } => f.write_str(what),
            Error::Full => f.write_str("the session is full; :reset starts again"),
        }
    }
}
