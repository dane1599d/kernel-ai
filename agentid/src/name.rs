//! An agent's name: the identity a person reads, and the one the action log keeps.
//!
//! A badge names an agent for one boot (the supervisor mints it at spawn), so the
//! log, `:why`, the panel and `undo <agent>` use the name, which is a package's name
//! under ADR 0011's shape: 2 to 12 of `a-z`, `0-9` and `-`, starting with a letter.
//! That shape holds no space, `=` or newline, so a name can never split or forge a
//! log record.

use alloc::string::{String, ToString};
use core::fmt;

/// The fewest bytes in a name.
pub const MIN_LEN: usize = 2;
/// The most bytes in a name.
pub const MAX_LEN: usize = 12;

/// An agent's name, checked.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(String);

/// Text that is not an agent's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadName;

impl fmt::Display for BadName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an agent's name is 2-12 of a-z, 0-9 and -, starting with a letter")
    }
}

impl Name {
    /// `text` as a name, if it has the shape.
    pub fn new(text: &str) -> Result<Self, BadName> {
        match shaped(text) {
            true => Ok(Self(text.to_string())),
            false => Err(BadName),
        }
    }

    /// A built-in agent's name, which is known to have the shape.
    pub(crate) fn builtin(text: &'static str) -> Self {
        debug_assert!(shaped(text));
        Self(text.to_string())
    }

    /// The person's own agent: the shell, which is you (ADR 0013 section 1).
    pub fn kai() -> Self {
        Self(String::from("kai"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the person's own agent.
    pub fn is_kai(&self) -> bool {
        self.0 == "kai"
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn shaped(text: &str) -> bool {
    let bytes = text.as_bytes();
    (MIN_LEN..=MAX_LEN).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}
