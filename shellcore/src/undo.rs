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

/// How to put one change back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inverse {
    /// Install this policy and preset again.
    Spec { policy: u64, preset: u64 },
}

/// One change a script made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Done {
    pub function: &'static str,
    /// What changed, in code's words: `policy round-robin (was interactive)`.
    pub what: String,
    pub inverse: Option<Inverse>,
}

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

/// Puts one change back; what it did, or why it could not.
pub fn revert(host: &dyn Host, inverse: Inverse) -> Result<String, String> {
    match inverse {
        Inverse::Spec { policy, preset } => host
            .set_spec(policy, preset)
            .map(|()| format!("policy back to {}", spec_name(policy, preset)))
            .map_err(|error| format!("cannot switch the policy back yet ({error:?})")),
    }
}
