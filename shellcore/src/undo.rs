//! What a script changed, recorded as it happens with the value it replaced, so
//! `undo` can put it back. A change with no recorded inverse cannot be run-tier.

use crate::Host;
use crate::tune::spec_of;
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
    }
}

/// Puts one change back; what it did, or why it did not. Compare-and-set: a value
/// that moved since the change is left alone unless `anyway`. A switch the kernel
/// refuses as too soon is tried once more after [`BUSY_RETRY_MS`].
pub fn revert(host: &dyn Host, inverse: Inverse, anyway: bool) -> Result<String, Refused> {
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
    }
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
    }
}
