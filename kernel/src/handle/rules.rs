//! What each standing rule covers (ADR 0002 D2a, ADR 0001 D6). An "Always allow"
//! on the menu makes a `rule` grant node ([`crate::allow`]), and the kernel notes
//! here what the user allowed: the call's action for "every target"
//! ([`termcore::menu::Answer::AlwaysKind`]), or the call exactly as shown for
//! "this target" (`AlwaysTarget`). Whoever runs a call the user did not answer
//! this time, the kernel for PROC_STOP_ONCE and the supervisor through RULE_CHECK,
//! asks [`covers`]: whether the caller holds a live handle to a rule whose scope
//! covers the call. Revoking the node (`rules_remove`) ends the rule.

use super::HANDLES;
use super::grant::Kernel;
use alloc::string::String;
use alloc::vec::Vec;
use captable::grant::{GrantRef, Live};
use kabi::handle::Kind;
use spin::Mutex;

/// Rules alive at once; a dead one's place is reused.
const MAX_RULES: usize = 64;

/// What a rule allows.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Scope {
    /// Every call of this action: `process_stop`.
    Action(String),
    /// This call only, as [`digest`] of its canonical text.
    Call([u8; 32]),
}

static RULES: Mutex<Vec<(GrantRef, Scope)>> = Mutex::new(Vec::new());

/// Notes that the rule node `node` allows `call`'s action (`every`) or `call`
/// itself. Busy when [`MAX_RULES`] live rules are noted already.
pub fn record(node: GrantRef, call: &str, every: bool) -> Result<(), kabi::Error> {
    let scope = match every {
        true => Scope::Action(String::from(action(call))),
        false => Scope::Call(digest(call)),
    };
    let mut rules = RULES.lock();
    rules.retain(|(node, _)| Kernel.alive(*node));
    if rules.len() >= MAX_RULES {
        return Err(kabi::Error::Busy);
    }
    rules.try_reserve(1).map_err(|_| kabi::Error::NoMemory)?;
    rules.push((node, scope));
    Ok(())
}

/// Whether the process in `slot` holds a live handle to a rule that covers
/// `call`, in canonical text.
pub fn covers(slot: usize, call: &str) -> bool {
    // The nodes first, under the table's lock alone; then the rules.
    let nodes: Vec<GrantRef> = match HANDLES[slot].lock().as_ref() {
        Some(handles) => handles
            .table
            .iter()
            .filter(|(_, e)| e.kind == Kind::Grant)
            .filter_map(|(_, e)| e.grant)
            .filter(|node| Kernel.alive(*node))
            .collect(),
        None => return false,
    };
    let (action, digest) = (action(call), digest(call));
    RULES.lock().iter().any(|(node, scope)| {
        nodes.contains(node)
            && match scope {
                Scope::Action(allowed) => allowed == action,
                Scope::Call(allowed) => *allowed == digest,
            }
    })
}

/// A call's action: `process_stop` of `process_stop("build-3")`.
fn action(call: &str) -> &str {
    call.split_once('(').map_or(call, |(action, _)| action)
}

/// The digest a rule for exactly `call` keeps (`kai-rule 1`).
fn digest(call: &str) -> [u8; 32] {
    let mut sha = termcore::once::Sha256::new();
    sha.update(b"kai-rule 1\n");
    sha.update(call.as_bytes());
    sha.finish()
}
