//! The agent's name in every log entry (ADR 0013 section 1: `actlog 2`).
//!
//! `actlog 2` puts `agent=<name>` on an `E` record. The name comes from the caller
//! the kernel's badge named, never from the request or the outcome text, and an entry
//! cannot be made without a [`Stamp`]. A record written before the field existed is
//! `actlog 1`'s: the person's own shell wrote it, so a reader gives it the name `kai`.

use crate::name::Name;
use crate::roster::Caller;
use alloc::format;
use alloc::string::String;

/// The key of the field.
pub const KEY: &str = "agent";

/// Whose a log entry is. Only a caller makes one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    name: Name,
}

/// Why a field is not an `agent=` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldError {
    /// Not `agent=<name>`.
    NotAgentField,
    /// The value is not an agent's name.
    BadName,
}

impl Stamp {
    pub(crate) fn of(caller: &Caller) -> Self {
        Self {
            name: caller.name().clone(),
        }
    }

    pub fn name(&self) -> &Name {
        &self.name
    }

    /// The field as the record holds it: `agent=goals`.
    pub fn field(&self) -> String {
        format!("{KEY}={}", self.name)
    }
}

/// The agent a record's `agent=` field names; `None`, a record with no such field
/// (`actlog 1`'s), is the person's shell.
pub fn read_field(word: Option<&str>) -> Result<Name, FieldError> {
    let Some(word) = word else {
        return Ok(Name::kai());
    };
    let value = word
        .strip_prefix(KEY)
        .and_then(|rest| rest.strip_prefix('='))
        .ok_or(FieldError::NotAgentField)?;
    Name::new(value).map_err(|_| FieldError::BadName)
}

/// The entry `undo <agent>` reverts: the newest live one `agent` made, of
/// `(id, owner, live)` entries. The log's own checks (a later entry on the same
/// resource) come after this.
pub fn last_live<'a>(
    entries: impl IntoIterator<Item = (u64, &'a Name, bool)>,
    agent: &str,
) -> Option<u64> {
    entries
        .into_iter()
        .filter(|(_, owner, live)| *live && owner.as_str() == agent)
        .map(|(id, _, _)| id)
        .max()
}
