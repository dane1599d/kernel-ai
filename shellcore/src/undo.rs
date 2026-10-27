//! What a script changed, recorded as it happens with the value it replaced, so
//! `undo` can put it back. A change with no recorded inverse cannot be run-tier.

use crate::tune::spec_of;
use crate::{Host, Proof};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use kabi::policy;

/// How long to wait before trying a refused switch once more: the kernel takes a
/// policy switch at most every 100 ms.
const BUSY_RETRY_MS: u64 = 100;

/// How to put one change back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inverse {
    /// Install `policy` and `preset` again, but only while `after` (the policy and
    /// preset the change installed) is still what runs.
    Spec {
        policy: u64,
        preset: u64,
        after: (u64, u64),
    },
    /// Stop the service a run started again.
    Started(Name),
    /// Start the service an undo stopped again: the redo of [`Inverse::Started`].
    Stopped(Name),
    /// Make package generation `to` current again, but only while `from` (the one
    /// the switch made) still is. The shell's package store does it
    /// (`Shell::revert_entry`), not the host.
    Generation { from: u64, to: u64 },
}

/// A service name held without the heap, so an inverse stays `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; kabi::MAX_NAME],
    len: u8,
}

impl Name {
    /// `None` for a name longer than a process name can be.
    pub fn new(name: &str) -> Option<Self> {
        let mut bytes = [0u8; kabi::MAX_NAME];
        bytes
            .get_mut(..name.len())?
            .copy_from_slice(name.as_bytes());
        Some(Self {
            bytes,
            len: name.len() as u8,
        })
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }
}

/// Why a change was not put back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Something else changed it since, to this (in the user's words); putting the
    /// old value back would overwrite that, so the user is asked first.
    Moved(String),
    /// It could not be put back yet.
    Failed(String),
}

/// One change a script made, as the action log records it.
pub type Done = crate::actlog::Change;

/// The changes of one run, in order.
pub type Journal = Rc<RefCell<Vec<Done>>>;

/// The policy and preset installed now, to record before changing them. `None`
/// for a policy outside the catalog, which cannot be installed again.
pub fn current_spec(host: &dyn Host) -> Option<(u64, u64)> {
    let (policy, preset) = match host.sched_stats(0) {
        Ok(stats) => (stats.policy, stats.preset),
        Err(_) => (host.metrics().ok()?.policy, kabi::sched::preset::GENTLE),
    };
    (policy != policy::OTHER).then_some((policy, preset))
}

/// `round-robin`, `classes/firm`: a policy as the user sees it.
pub fn spec_name(policy: u64, preset: u64) -> String {
    format!("{}", spec_of(policy, preset).label())
}

/// What `inverse` puts back, in the user's words: `interactive`.
pub fn target(inverse: Inverse) -> String {
    match inverse {
        Inverse::Spec { policy, preset, .. } => spec_name(policy, preset),
        Inverse::Started(name) => format!("{} stopped", name.as_str()),
        Inverse::Stopped(name) => format!("{} running", name.as_str()),
        Inverse::Generation { to, .. } => format!("generation {to}"),
    }
}

/// Puts one change back; what it did, or why it did not. Compare-and-set: a value
/// that moved since the change is left alone unless `anyway`. A switch the kernel
/// refuses as too soon is tried once more after [`BUSY_RETRY_MS`].
pub fn revert(
    host: &dyn Host,
    inverse: Inverse,
    anyway: bool,
    ids: Ids,
) -> Result<String, Refused> {
    match inverse {
        Inverse::Spec {
            policy,
            preset,
            after,
        } => {
            let now = current_spec(host);
            if !anyway && now != Some(after) {
                let now = now.map_or_else(|| String::from("unknown"), |(p, s)| spec_name(p, s));
                return Err(Refused::Moved(now));
            }
            let mut result = host.set_spec(policy, preset);
            if result == Err(kabi::Error::Busy) {
                host.sleep_ms(BUSY_RETRY_MS);
                result = host.set_spec(policy, preset);
            }
            result
                .map(|()| format!("policy back to {}", spec_name(policy, preset)))
                .map_err(|error| {
                    Refused::Failed(format!("cannot switch the policy back yet ({error:?})"))
                })
        }
        Inverse::Started(name) => host
            .stop_service(name.as_str(), Proof::Undo { id: ids.made })
            .map(|()| format!("service {} stopped again", name.as_str()))
            .map_err(Refused::Failed),
        Inverse::Stopped(name) => host
            .start_service(name.as_str(), ids.next)
            .map(|()| format!("service {} started again", name.as_str()))
            .map_err(Refused::Failed),
        Inverse::Generation { .. } => Err(Refused::Failed(String::from(
            "a package switch is put back by the shell's store",
        ))),
    }
}

/// The log entries a revert stands between: `made`, whose change it puts back,
/// and `next`, the entry the revert itself is logged as.
#[derive(Clone, Copy, Debug)]
pub struct Ids {
    pub made: u64,
    pub next: u64,
}

/// How to put back what reverting with `inverse` did: the redo.
pub fn reverse(inverse: Inverse) -> Inverse {
    match inverse {
        Inverse::Spec {
            policy,
            preset,
            after,
        } => Inverse::Spec {
            policy: after.0,
            preset: after.1,
            after: (policy, preset),
        },
        Inverse::Started(name) => Inverse::Stopped(name),
        Inverse::Stopped(name) => Inverse::Started(name),
        Inverse::Generation { from, to } => Inverse::Generation { from: to, to: from },
    }
}
