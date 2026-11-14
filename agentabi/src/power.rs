//! The restart row (founder test 2, backlog item 13): the machine restarted
//! from inside the OS, through the kernel's `POWER_REBOOT` (`cap::POWER`, the
//! shell's plan word `power`). It always asks: a restart stops every program,
//! and nothing undoes it. Only the person's own line reaches it: `restart`,
//! `reboot` or a few plain phrasings, caught before the model
//! (`shellcore::power`); a question about restarting is not one of them.

use crate::{Action, Facts, Inverse, Owner, RetType, Returns, Tier};
use kabi::cap;

/// `power_reboot()`: restarts the computer once the person says yes. The shell
/// keeps its own log first; `fs` has made every answered change durable.
pub const POWER_REBOOT: Action = Action {
    name: "power_reboot",
    doc: "restarts the computer; it always asks",
    args: &[],
    // Every running program stops, and what it held in memory is lost.
    facts: Facts {
        destructive: true,
        idempotent: false,
        ..Facts::LOCAL_CHANGE
    },
    inverse: Inverse::None("a restart cannot be undone"),
    cues: &[
        "reboot",
        "restart the computer",
        "restart the machine",
        "restart the pc",
        "restart kai",
    ],
    returns: Returns::System,
    ret: RetType::Unit,
    // The shell asks, then calls the kernel with its `cap::POWER`. Shell-owned,
    // so it is in no pack (the act pack is at its 980-token bar): the shell's
    // own words catch a restart line before the model, and a model's call to it
    // finds no row to run.
    owner: Owner::Shell,
    needs: cap::POWER,
    min_tier: Tier::Ask,
    old_name: None,
};

/// Rows the shell carries out itself through the kernel's power calls
/// (`shellcore::power`).
pub const POWER_ROWS: [Action; 1] = [POWER_REBOOT];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive;

    #[test]
    fn a_restart_always_asks() {
        assert_eq!(derive(&POWER_REBOOT), Tier::Ask);
        assert_eq!(POWER_REBOOT.needs, cap::POWER);
    }
}
