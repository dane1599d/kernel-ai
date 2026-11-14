//! What a plan commits to the kernel's admission budget at boot. Every live
//! process's `max_pages` counts against the budget (`free / 2` at boot), so a
//! plan whose boot services nearly fill it fails on the next few pages a boot
//! image or a quota grows by, as `[supervisor] cannot start shell: NoMemory`.

use crate::{Plan, Start};

/// Pages in a MiB.
pub const PAGES_PER_MIB: u64 = 256;
/// Below this much room left after the boot services, the kernel warns: a
/// slightly bigger image or quota would refuse a service.
pub const MIN_ROOM_MIB: u64 = 16;

/// The pages the services started at boot commit, the supervisor's own
/// `supervisor_pages` included; `start=pkg` and `start=demand` lines start
/// later and are not counted.
pub fn boot_pages(plan: &Plan, supervisor_pages: u64) -> u64 {
    plan.services
        .iter()
        .filter(|service| service.start == Start::Boot)
        .map(|service| service.limits.max_pages)
        .fold(supervisor_pages, u64::saturating_add)
}

/// Whether `committed` pages leave under [`MIN_ROOM_MIB`] of `budget`.
pub fn tight(committed: u64, budget: u64) -> bool {
    budget.saturating_sub(committed) < MIN_ROOM_MIB * PAGES_PER_MIB
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_plan;

    #[test]
    fn counts_boot_services_and_the_supervisor_only() {
        let plan = parse_plan(
            "fs pages=8192\nshell pages=4096\nnotes pages=1024 start=demand\nextra start=pkg",
        );
        assert!(plan.errors.is_empty(), "{:?}", plan.errors);
        assert_eq!(boot_pages(&plan, 2048), 2048 + 8192 + 4096);
    }

    #[test]
    fn a_line_without_pages_counts_the_default_quota() {
        let plan = parse_plan("llm");
        assert_eq!(boot_pages(&plan, 0), crate::DEFAULT_PAGES);
    }

    #[test]
    fn under_sixteen_mib_of_room_is_tight() {
        let budget = 26_880;
        // desk-catalog on 512 MiB (#877): 26,624 pages of about 26,880.
        assert!(tight(26_624, budget));
        assert!(tight(budget + 1, budget));
        assert!(!tight(budget - MIN_ROOM_MIB * PAGES_PER_MIB, budget));
    }
}
