//! The `maint` agent's rules (ADR 0013 section 7; row X1 of
//! `docs/plans/agents-os.md`): it reads the machine's health and proposes, and
//! it applies nothing. A proposal is a line for the panel and a read the person
//! may run (`:fault <service>`, the log's own door); every change still goes
//! through the one door, at its own tier, from the person's line. So this crate
//! has no call to make, and holds no handle.
//!
//! What it watches, as the OS half feeds it ([`Signal`]): a service the supervisor
//! restarted, and a fault a service reported. A crash loop is
//! [`LOOP_RESTARTS`] restarts within [`LOOP_WINDOW_MS`]: one proposal names it,
//! and the same service is not proposed again for [`COOLDOWN_MS`], so a loop
//! cannot flood the panel. A fault proposes once per service per cooldown too.
//!
//! The names come from the supervisor's records, not from a model or a file, but
//! they are held to a service name's shape all the same, and what is kept is
//! bounded: at most [`MAX_SERVICES`] services, and per service only the restarts
//! inside the window.

#![no_std]

extern crate alloc;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::String;

/// Restarts within [`LOOP_WINDOW_MS`] that make a crash loop.
pub const LOOP_RESTARTS: usize = 3;
/// The window a crash loop is counted in: 10 minutes.
pub const LOOP_WINDOW_MS: u64 = 10 * 60 * 1000;
/// How long after a proposal the same service is not proposed again: an hour.
pub const COOLDOWN_MS: u64 = 60 * 60 * 1000;
/// The most services watched at once; a new one past it is not watched.
pub const MAX_SERVICES: usize = 64;
/// The longest service name kept.
pub const MAX_NAME: usize = 32;

/// What the OS half saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signal {
    /// The supervisor restarted `service` at `at_ms`.
    Restart { service: String, at_ms: u64 },
    /// `service` reported a fault at `at_ms` (an `S` record, a panic it logged).
    Fault { service: String, at_ms: u64 },
}

/// Why `maint` proposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    CrashLoop,
    Fault,
}

/// A line for the panel, and the read the person may run. Never an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    pub service: String,
    pub why: Why,
    pub line: String,
    /// A read the person may type: `:fault <service>`.
    pub read: String,
}

/// A service name as the supervisor's plan has it: 1 to [`MAX_NAME`] of `a-z`,
/// `0-9`, `-` and `_`.
pub fn is_service_name(name: &str) -> bool {
    (1..=MAX_NAME).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// What `maint` has seen.
#[derive(Clone, Debug, Default)]
pub struct Watch {
    /// Each service's restarts inside the window, oldest first.
    restarts: BTreeMap<String, VecDeque<u64>>,
    /// When each service was last proposed.
    proposed: BTreeMap<String, u64>,
}

impl Watch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes `signal` in: a proposal when it makes one.
    pub fn observe(&mut self, signal: Signal) -> Option<Proposal> {
        let (service, at_ms, why) = match signal {
            Signal::Restart { service, at_ms } => (service, at_ms, Why::CrashLoop),
            Signal::Fault { service, at_ms } => (service, at_ms, Why::Fault),
        };
        if !is_service_name(&service) {
            return None;
        }
        if why == Why::CrashLoop && !self.count_restart(&service, at_ms) {
            return None;
        }
        self.propose(service, at_ms, why)
    }

    /// Counts a restart: whether the service is now in a crash loop.
    fn count_restart(&mut self, service: &str, at_ms: u64) -> bool {
        if !self.restarts.contains_key(service) && self.restarts.len() >= MAX_SERVICES {
            return false;
        }
        let seen = self.restarts.entry(String::from(service)).or_default();
        // A clock that went back counts as now: the window is never longer.
        let at_ms = seen.back().map_or(at_ms, |last| at_ms.max(*last));
        seen.push_back(at_ms);
        while seen
            .front()
            .is_some_and(|first| at_ms - first >= LOOP_WINDOW_MS)
        {
            seen.pop_front();
        }
        while seen.len() > LOOP_RESTARTS {
            seen.pop_front();
        }
        seen.len() >= LOOP_RESTARTS
    }

    fn propose(&mut self, service: String, at_ms: u64, why: Why) -> Option<Proposal> {
        if self
            .proposed
            .get(&service)
            .is_some_and(|last| at_ms.saturating_sub(*last) < COOLDOWN_MS)
        {
            return None;
        }
        if !self.proposed.contains_key(&service) && self.proposed.len() >= MAX_SERVICES {
            return None;
        }
        self.proposed.insert(service.clone(), at_ms);
        let line = match why {
            Why::CrashLoop => format!(
                "{service} restarted {LOOP_RESTARTS} times in {} minutes. Look at why with :fault {service}?",
                LOOP_WINDOW_MS / 60_000
            ),
            Why::Fault => format!("{service} reported a fault. Look at it with :fault {service}?"),
        };
        Some(Proposal {
            read: format!(":fault {service}"),
            service,
            why,
            line,
        })
    }

    /// Services watched for restarts now.
    pub fn watched(&self) -> usize {
        self.restarts.len()
    }
}

#[cfg(test)]
mod tests;
