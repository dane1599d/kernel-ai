//! The `goals` agent's ceiling, one goal at a time (ADR 0013 sections 1 and 5).
//!
//! A built-in agent other than `kai` holds no grants of its own
//! ([`crate::Agent::allows`] is false for it), so `goals` is refused at the door
//! for every row until the person approves a goal. Then:
//!
//! - **The person sets it, with `goal_set`'s one ask.** [`Roster::grant_goal`]
//!   takes the person's [`Mark`] from the key that answered that ask, and the
//!   caller must be `kai`, the shell that runs the approved `goal_set`. Nothing an
//!   agent typed or a model wrote can make one.
//! - **It holds only the rows that goal's scope names**, each resolved against the
//!   registry (a row, or `<group>_*`, as a goal's scope writes them), and only for
//!   that goal: [`Roster::admit_for_goal`] admits `goals` for a row of the goal it
//!   names and no other.
//! - **It never covers a send or an install row.** A scope that names one, or a
//!   group that holds one, is refused whole ([`GrantRefused::SendsOrInstalls`]): a
//!   change that reaches past the machine (`open_world` and not read-only) or that
//!   installs, removes or switches code or a model (`INSTALL_ROWS`), or that changes
//!   what apps or goals may reach (`PERMISSION_ROWS`). The person can still do those with
//!   their own line.
//! - **It ends with the goal.** [`Roster::revoke_goal`] drops it, and a call that
//!   waited (for a yes, in a queue) is refused when it is checked again.

use crate::door::{Admission, Denied};
use crate::mark::Mark;
use crate::roster::{Builtin, Caller, Roster};
use agentabi::{Action, Tier};
use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// Rows that install, remove or switch code or a model: never in a goal's ceiling.
/// `component_revert` swaps a live component to an earlier version; `tune` runs a
/// kernel scheduler trial and `tune_undo` swaps back the policy a trial kept. A test
/// walks every row of the registry, so a new row of this kind cannot slip past.
pub const INSTALL_ROWS: &[&str] = &[
    "pkg_install",
    "pkg_remove",
    "app_build",
    "app_install",
    "model_fetch",
    "model_use",
    "component_revert",
    "tune",
    "tune_undo",
];
/// Rows that change what apps or goals may reach (the person's site grants, the
/// goals themselves, and the memories Kai reads every turn): never in a goal's
/// ceiling, since a goal that could restore a grant, set or resume a goal, or
/// write a memory that steers later turns could widen itself.
pub const PERMISSION_ROWS: &[&str] = &[
    "grant_revoke",
    "grant_restore",
    "goal_set",
    "goal_pause",
    "goal_resume",
    "goal_cancel",
    "memory_remember",
    "memory_edit",
    "memory_forget",
    "memory_forget_all",
];
/// The most goals that hold a ceiling at once (goalcore keeps at most this many).
pub const MAX_GOAL_GRANTS: usize = 16;

/// Why a goal's ceiling was not set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantRefused {
    /// Only `kai`, running the approved `goal_set`, sets one.
    NotKai,
    /// The key that answered was not a person's.
    NotThePerson,
    /// A scope entry names no row of the registry.
    UnknownRow(String),
    /// A scope entry is, or covers, a row that sends or installs.
    SendsOrInstalls(String),
    /// A scope entry is, or covers, a row that changes what apps may reach.
    Permissions(String),
    /// A scope entry is a row that never runs.
    Never(String),
    /// The scope names no row.
    Empty,
    /// [`MAX_GOAL_GRANTS`] goals hold one already.
    Full,
}

impl fmt::Display for GrantRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotKai => f.write_str("only the person's own approval sets a goal's actions"),
            Self::NotThePerson => f.write_str("a goal's actions are set by the person's own yes"),
            Self::UnknownRow(r) => write!(f, "{r}: no such action"),
            Self::SendsOrInstalls(r) => write!(
                f,
                "{r}: a goal may not send or install; do that with your own line"
            ),
            Self::Permissions(r) => write!(
                f,
                "{r}: a goal may not change what apps or goals may reach; do that with your own line"
            ),
            Self::Never(r) => write!(f, "{r}: that action never runs"),
            Self::Empty => f.write_str("a goal needs at least one action"),
            Self::Full => write!(f, "at most {MAX_GOAL_GRANTS} goals hold actions at once"),
        }
    }
}

/// Whether `action` sends past the machine or installs: never a goal's.
pub fn sends_or_installs(action: &Action) -> bool {
    (action.facts.open_world && !action.facts.read_only) || INSTALL_ROWS.contains(&action.name)
}

/// The registry's rows that scope entry `entry` names: the row, or every row of
/// `<group>_*`.
fn resolve<'r>(entry: &str, rows: &'r [Action]) -> Vec<&'r Action> {
    match entry.strip_suffix('*') {
        Some(prefix) if prefix.ends_with('_') && prefix.len() > 1 => rows
            .iter()
            .filter(|action| action.name.starts_with(prefix))
            .collect(),
        _ => agentabi::find(rows, entry).into_iter().collect(),
    }
}

impl Roster {
    /// Sets goal `goal`'s ceiling to the rows `scope` names (resolved in `rows`),
    /// for the `goals` agent, from the person's yes to `goal_set`'s ask: `by` must
    /// be `kai` and `mark` the person's. The registry names it holds, sorted.
    pub fn grant_goal(
        &mut self,
        by: &Caller,
        mark: Mark,
        goal: u64,
        scope: &[String],
        rows: &[Action],
    ) -> Result<Vec<String>, GrantRefused> {
        if !(by.is_kai() && self.is_live(by)) {
            return Err(GrantRefused::NotKai);
        }
        if !mark.is_person() {
            return Err(GrantRefused::NotThePerson);
        }
        let mut granted = BTreeSet::new();
        for entry in scope {
            let named = resolve(entry, rows);
            if named.is_empty() {
                return Err(GrantRefused::UnknownRow(entry.clone()));
            }
            for action in named {
                if sends_or_installs(action) {
                    return Err(GrantRefused::SendsOrInstalls(action.name.to_string()));
                }
                if PERMISSION_ROWS.contains(&action.name) {
                    return Err(GrantRefused::Permissions(action.name.to_string()));
                }
                if agentabi::derive(action) == Tier::Never {
                    return Err(GrantRefused::Never(action.name.to_string()));
                }
                granted.insert(action.name.to_string());
            }
        }
        if granted.is_empty() {
            return Err(GrantRefused::Empty);
        }
        if !self.goal_grants.contains_key(&goal) && self.goal_grants.len() >= MAX_GOAL_GRANTS {
            return Err(GrantRefused::Full);
        }
        let list: Vec<String> = granted.into_iter().collect();
        self.goal_grants.insert(goal, list.clone());
        Ok(list)
    }

    /// Drops goal `goal`'s ceiling, when it ends, is cancelled or is forgotten.
    pub fn revoke_goal(&mut self, goal: u64) -> bool {
        self.goal_grants.remove(&goal).is_some()
    }

    /// The rows goal `goal` may call, if it holds a ceiling.
    pub fn goal_rows(&self, goal: u64) -> Option<&[String]> {
        self.goal_grants.get(&goal).map(Vec::as_slice)
    }

    /// Whether `caller`, the `goals` agent, may call `row` for goal `goal`, against
    /// the OS's own rows.
    pub fn admit_for_goal(
        &self,
        caller: &Caller,
        goal: u64,
        row: &str,
    ) -> Result<Admission, Denied> {
        self.admit_for_goal_in(caller, goal, row, &agentabi::ROWS)
    }

    /// [`Roster::admit_for_goal`] against `rows`. Only `goals` is admitted this way,
    /// and only for a row its goal's ceiling holds.
    pub fn admit_for_goal_in(
        &self,
        caller: &Caller,
        goal: u64,
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
        let is_goals = self
            .get(caller.id())
            .is_some_and(|agent| matches!(agent.kind(), crate::Kind::Builtin(Builtin::Goals)));
        let held = self
            .goal_grants
            .get(&goal)
            .is_some_and(|names| names.iter().any(|name| name == action.name));
        // A send, install or permission row is refused even if a ceiling named it.
        let never = sends_or_installs(action) || PERMISSION_ROWS.contains(&action.name);
        if !(is_goals && held) || never {
            return Err(Denied::OutsideGrants);
        }
        Ok(Admission::for_goal(caller.clone(), action.name, goal))
    }
}

#[cfg(test)]
mod tests;
