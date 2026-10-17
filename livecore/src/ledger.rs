//! The kernel's record of one component: which version is active, that version's
//! last state, and every swap. A host reports events; the kernel applies them here
//! and refuses any the rules do not allow, so the record cannot be talked into a
//! state the host could not have reached.
//!
//! The invariant: the checkpoint always belongs to the active version, so a host
//! that dies at any moment restarts on the active version with its data.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::{Reason, flag, kind};

/// Entries kept in [`Ledger::history`].
pub const HISTORY: usize = 32;
/// Most versions on the failed list.
const MAX_BAD: usize = 16;

/// An event as the kernel received it, already copied out of the host's memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub kind: u64,
    pub reason: u64,
    pub label: String,
    pub schema: u64,
    pub served: u64,
    pub regressions: u64,
    pub flags: u64,
    pub blob: Vec<u8>,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Empty,
    Steady {
        active: String,
    },
    Swapping {
        active: String,
        candidate: String,
        begin_pid: u64,
    },
}

/// Counts, for the terminal and the self-test.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub restores: u64,
    pub checkpoints: u64,
    pub begins: u64,
    pub commits: u64,
    pub rollbacks: u64,
    /// Rollbacks by [`Reason`], indexed by its number.
    pub by_reason: [u64; 16],
    pub faults: u64,
    /// Faults after which the version was reloaded in place.
    pub restarts: u64,
    /// Swaps to a version that failed before, refused without `force`.
    pub refused_bad: u64,
    /// Events the rules refused.
    pub rejected: u64,
    pub disabled: u64,
    pub resets: u64,
    /// Requests served by candidates on probation.
    pub served: u64,
    pub regressions: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub seq: u64,
    pub kind: u64,
    pub reason: u64,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct Ledger {
    pub phase: Phase,
    /// The checkpoint's schema: always the active version's.
    pub schema: u64,
    pub checkpoint: Vec<u8>,
    /// Commits so far.
    pub generation: u64,
    pub disabled: bool,
    /// The process that last restored the component.
    pub host_pid: u64,
    /// The versions the current host fetched; it may only report on those.
    fetched: (u64, Vec<String>),
    /// Versions that failed, with why; retrying one needs `force`.
    pub bad: Vec<(String, Reason)>,
    pub counters: Counters,
    pub history: VecDeque<HistoryEntry>,
    seq: u64,
    pub last_detail: String,
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}

impl Ledger {
    pub const fn new() -> Self {
        Self {
            phase: Phase::Empty,
            schema: 0,
            checkpoint: Vec::new(),
            generation: 0,
            disabled: false,
            host_pid: 0,
            fetched: (0, Vec::new()),
            bad: Vec::new(),
            counters: Counters {
                restores: 0,
                checkpoints: 0,
                begins: 0,
                commits: 0,
                rollbacks: 0,
                by_reason: [0; 16],
                faults: 0,
                restarts: 0,
                refused_bad: 0,
                rejected: 0,
                disabled: 0,
                resets: 0,
                served: 0,
                regressions: 0,
            },
            history: VecDeque::new(),
            seq: 0,
            last_detail: String::new(),
        }
    }

    pub fn active(&self) -> Option<&str> {
        match &self.phase {
            Phase::Empty => None,
            Phase::Steady { active } | Phase::Swapping { active, .. } => Some(active),
        }
    }

    pub fn candidate(&self) -> Option<&str> {
        match &self.phase {
            Phase::Swapping { candidate, .. } => Some(candidate),
            _ => None,
        }
    }

    /// Notes that process `pid` fetched version `label`.
    pub fn fetched(&mut self, pid: u64, label: &str) {
        if self.fetched.0 != pid {
            self.fetched = (pid, Vec::new());
        }
        if !self.fetched.1.iter().any(|l| l == label) {
            self.fetched.1.push(label.to_string());
        }
    }

    fn has_fetched(&self, pid: u64, label: &str) -> bool {
        self.fetched.0 == pid && self.fetched.1.iter().any(|l| l == label)
    }

    pub fn is_bad(&self, label: &str) -> Option<Reason> {
        self.bad
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, why)| *why)
    }

    /// Applies an event from process `pid`. On success, the line the kernel logs.
    pub fn apply(&mut self, pid: u64, event: &Event) -> Result<Option<String>, Error> {
        match self.rule(pid, event) {
            Ok(line) => {
                if event.kind != kind::CHECKPOINT {
                    self.remember(event);
                }
                Ok(line)
            }
            Err(error) => {
                if error == Error::Invalid {
                    self.counters.rejected += 1;
                }
                Err(error)
            }
        }
    }

    fn remember(&mut self, event: &Event) {
        self.seq += 1;
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(HistoryEntry {
            seq: self.seq,
            kind: event.kind,
            reason: event.reason,
            label: event.label.clone(),
        });
        if !event.detail.is_empty() {
            self.last_detail = event.detail.clone();
        }
    }

    /// The header of `blob` must name `schema`.
    fn check_blob(blob: &[u8], schema: u64) -> Result<(), Error> {
        match crate::state::schema_of(blob) {
            Some(found) if found == schema && schema > 0 => Ok(()),
            _ => Err(Error::Invalid),
        }
    }

    fn rule(&mut self, pid: u64, event: &Event) -> Result<Option<String>, Error> {
        let label = event.label.as_str();
        let reason = Reason::from_raw(event.reason);
        // Only events about a version carry a label, and those labels are checked
        // against what the host fetched or the candidate; the rest carry none.
        let labelled = matches!(
            event.kind,
            kind::RESTORED | kind::BEGIN | kind::COMMIT | kind::ROLLBACK
        );
        if !labelled && !label.is_empty() {
            return Err(Error::Invalid);
        }
        match (event.kind, &self.phase) {
            (kind::RESTORED, Phase::Empty) if !self.disabled => {}
            (kind::RESTORED, Phase::Steady { active }) if active == label && !self.disabled => {}
            (kind::RESTORED, _) => return Err(Error::Invalid),
            (kind::CHECKPOINT, Phase::Steady { .. } | Phase::Swapping { .. })
                if pid == self.host_pid =>
            {
                Self::check_blob(&event.blob, self.schema)?;
                self.checkpoint = event.blob.clone();
                self.counters.checkpoints += 1;
                return Ok(None);
            }
            (kind::BEGIN, Phase::Steady { active })
                if pid == self.host_pid && active != label && !self.disabled =>
            {
                if !self.has_fetched(pid, label) {
                    return Err(Error::Invalid);
                }
                Self::check_blob(&event.blob, self.schema)?;
                if let Some(why) = self.is_bad(label)
                    && event.flags & flag::FORCE == 0
                {
                    self.counters.refused_bad += 1;
                    self.last_detail = format!("{label} failed before ({})", why.name());
                    return Err(Error::Permission);
                }
                let active = active.clone();
                self.phase = Phase::Swapping {
                    active: active.clone(),
                    candidate: label.to_string(),
                    begin_pid: pid,
                };
                self.checkpoint = event.blob.clone();
                self.counters.begins += 1;
                return Ok(Some(format!("{active} -> {label} swap begins")));
            }
            (
                kind::COMMIT,
                Phase::Swapping {
                    candidate,
                    begin_pid,
                    active,
                },
            ) if candidate == label && *begin_pid == pid => {
                Self::check_blob(&event.blob, event.schema)?;
                let line = format!("{active} -> {label} committed after {} calls", event.served);
                self.bad.retain(|(l, _)| l != label);
                self.phase = Phase::Steady {
                    active: label.to_string(),
                };
                self.schema = event.schema;
                self.checkpoint = event.blob.clone();
                self.generation += 1;
                self.counters.commits += 1;
                self.counters.served = self.counters.served.saturating_add(event.served);
                self.counters.regressions =
                    self.counters.regressions.saturating_add(event.regressions);
                return Ok(Some(line));
            }
            (
                kind::ROLLBACK,
                Phase::Swapping {
                    candidate,
                    begin_pid,
                    active,
                },
            ) if candidate == label => {
                let reason = reason.ok_or(Error::Invalid)?;
                // Only a later host reports that the one that began the swap died.
                let from_new_host = *begin_pid != pid;
                if (reason == Reason::HostRestart) != from_new_host {
                    return Err(Error::Invalid);
                }
                let line = format!(
                    "{active} -> {label} rolled back ({}{}{})",
                    reason.name(),
                    if event.detail.is_empty() { "" } else { ": " },
                    event.detail
                );
                let blame = !matches!(reason, Reason::NoMigration | Reason::Manual)
                    && event.flags & flag::POISON == 0;
                if blame && self.is_bad(label).is_none() {
                    if self.bad.len() == MAX_BAD {
                        self.bad.remove(0);
                    }
                    self.bad.push((label.to_string(), reason));
                }
                self.phase = Phase::Steady {
                    active: active.clone(),
                };
                self.counters.rollbacks += 1;
                self.counters.by_reason[reason as usize] += 1;
                self.counters.served = self.counters.served.saturating_add(event.served);
                self.counters.regressions =
                    self.counters.regressions.saturating_add(event.regressions);
                return Ok(Some(line));
            }
            (kind::FAULT, Phase::Steady { active } | Phase::Swapping { active, .. })
                if pid == self.host_pid =>
            {
                let reason = reason.ok_or(Error::Invalid)?;
                self.counters.faults += 1;
                let restarted = event.flags & flag::RESTARTED != 0;
                if restarted {
                    self.counters.restarts += 1;
                }
                return Ok(Some(format!(
                    "{active} failed ({}){}",
                    reason.name(),
                    if restarted {
                        "; reloaded from its checkpoint"
                    } else {
                        ""
                    }
                )));
            }
            (kind::DISABLED, Phase::Steady { active }) if !self.disabled => {
                let reason = reason.ok_or(Error::Invalid)?;
                self.disabled = true;
                self.host_pid = pid;
                self.counters.disabled += 1;
                return Ok(Some(format!(
                    "{active} disabled: its state cannot be restored ({})",
                    reason.name()
                )));
            }
            // From the host that disabled it, or a later one that loaded the version.
            (kind::RESET, Phase::Steady { active })
                if self.disabled && (pid == self.host_pid || self.has_fetched(pid, active)) =>
            {
                let line = format!("{active} reset: its state was cleared");
                self.disabled = false;
                self.checkpoint.clear();
                self.schema = 0;
                self.phase = Phase::Empty;
                self.counters.resets += 1;
                return Ok(Some(line));
            }
            _ => return Err(Error::Invalid),
        }
        // RESTORED: the host runs `label` from this state from now on.
        if !self.has_fetched(pid, label) {
            return Err(Error::Invalid);
        }
        Self::check_blob(&event.blob, event.schema)?;
        let line = match &self.phase {
            Phase::Empty => format!("{label} started fresh"),
            _ => format!("{label} restored from its checkpoint"),
        };
        self.phase = Phase::Steady {
            active: label.to_string(),
        };
        self.schema = event.schema;
        self.checkpoint = event.blob.clone();
        self.host_pid = pid;
        self.counters.restores += 1;
        Ok(Some(line))
    }
}

#[cfg(test)]
mod tests;
