//! The one door and whose it is (ADR 0013 section 1, over ADR 0002 D2 and D3).
//!
//! Every call reaches `agentcore::execute` with an [`Admission`]: the roster's
//! answer to "may this caller call this row at all", made once, for the live caller,
//! from the agent's own grants. An agent's grants are the ceiling and the tiers decide
//! below it. The admission carries the caller, so the call and the log entry it
//! makes are both stamped with the same name, and it is the only thing that hands out
//! consent words:
//!
//! - **An agent never inherits the user's consent.** [`Admission::consent_words`] gives
//!   the line to `agentabi::consent::decide` only when the caller is `kai` and a person
//!   typed it. Every other caller gets the empty line, with which nothing is cued, no
//!   argument is in the line and the vague path has nothing to read, so every change
//!   asks and a read still runs. The agent's own request text, the person's earlier
//!   words quoted back to it and a goal's description are all just text.
//! - **An agent's ask is answered by the person.** See [`crate::asks`].

use crate::mark::Line;
use crate::roster::{Caller, Roster};
use crate::stamp::Stamp;
use agentabi::{Action, Tier};
use alloc::string::{String, ToString};
use core::fmt;

/// Why a caller may not call a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denied {
    /// The caller's agent is not on the roster any more (retired, or a later agent
    /// took the badge's place).
    Retired,
    /// No row has the name.
    UnknownRow,
    /// A row that never runs, for anyone.
    Never,
    /// The agent's own grants do not name the row.
    OutsideGrants,
}

impl fmt::Display for Denied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Retired => "that agent is not installed any more",
            Self::UnknownRow => "no such action",
            Self::Never => "that action never runs",
            Self::OutsideGrants => "the agent's grants do not include that action",
        })
    }
}

/// A caller let through the door for one row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Admission {
    caller: Caller,
    row: String,
}

impl Roster {
    /// Whether `caller` may call `row`, against the OS's own rows (`agentabi::ROWS`).
    pub fn admit(&self, caller: &Caller, row: &str) -> Result<Admission, Denied> {
        self.admit_in(caller, row, &agentabi::ROWS)
    }

    /// [`Roster::admit`] against `rows`, the registry the row must be in: the OS's
    /// and an installed app's. The registry must not let an app's row take an OS row's
    /// name (ADR 0011 section 2 reserves the groups), since the first match decides.
    pub fn admit_in(
        &self,
        caller: &Caller,
        row: &str,
        rows: &[Action],
    ) -> Result<Admission, Denied> {
        if !self.is_live(caller) {
            return Err(Denied::Retired);
        }
        let action = agentabi::find(rows, row).ok_or(Denied::UnknownRow)?;
        if agentabi::derive(action) == Tier::Never {
            return Err(Denied::Never);
        }
        let agent = self.get(caller.id()).ok_or(Denied::Retired)?;
        // The registry's own name, so a renamed row's old name is admitted as the new
        // one; the agent's grants may name either.
        if !agent.allows(action.name) && !agent.allows(row) {
            return Err(Denied::OutsideGrants);
        }
        Ok(Admission {
            caller: caller.clone(),
            row: action.name.to_string(),
        })
    }
}

impl Roster {
    /// Whether `admission` still holds: the caller is still on the roster and its grants
    /// still name the row. An admission is a moment's answer, and a call that waits
    /// (for a yes, in a queue) is checked again when it runs.
    pub fn revalidate(&self, admission: &Admission) -> Result<(), Denied> {
        self.revalidate_in(admission, &agentabi::ROWS)
    }

    /// [`Roster::revalidate`] against `rows`.
    pub fn revalidate_in(&self, admission: &Admission, rows: &[Action]) -> Result<(), Denied> {
        self.admit_in(&admission.caller, &admission.row, rows)
            .map(|_| ())
    }
}

impl Admission {
    pub fn caller(&self) -> &Caller {
        &self.caller
    }

    /// The row's registry name.
    pub fn row(&self) -> &str {
        &self.row
    }

    /// What the call's log entry is stamped with.
    pub fn stamp(&self) -> Stamp {
        Stamp::of(&self.caller)
    }

    /// The words `agentabi::consent::decide` may read as the user's line: `line`'s
    /// text if the caller is `kai` and a person typed it, else nothing. A line `kai`
    /// was given by an agent (text sent to the shell, keys from `ui_act`) is not the
    /// person's either.
    pub fn consent_words<'a>(&self, line: &Line<'a>) -> &'a str {
        match self.caller.is_kai() && line.mark().is_person() {
            true => line.text(),
            false => "",
        }
    }
}
