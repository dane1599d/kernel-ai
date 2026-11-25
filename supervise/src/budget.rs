//! What a plan commits to the kernel's admission budget at boot. Every live
//! process's `max_pages` counts against the budget (`free / 2` at boot), so a
//! plan whose boot services nearly fill it fails on the next few pages a boot
//! image or a quota grows by, as `[supervisor] cannot start shell: NoMemory`.

use crate::{Plan, Start};
use alloc::format;
use alloc::string::String;

/// Pages in a MiB.
pub const PAGES_PER_MIB: u64 = 256;
/// Below this much room left after the boot services, the kernel warns: a
/// slightly bigger image or quota would refuse a service.
pub const MIN_ROOM_MIB: u64 = 16;

/// The pages the services started at every boot commit, the supervisor's own
/// `supervisor_pages` included. `start=pkg` lines count: pkg starts the
/// installed apps at every boot. `start=demand` lines start only when asked
/// for and are not counted.
pub fn boot_pages(plan: &Plan, supervisor_pages: u64) -> u64 {
    plan.services
        .iter()
        .filter(|service| matches!(service.start, Start::Boot | Start::Pkg))
        .map(|service| service.limits.max_pages)
        .fold(supervisor_pages, u64::saturating_add)
}

/// Whether `committed` pages leave under [`MIN_ROOM_MIB`] of `budget`.
pub fn tight(committed: u64, budget: u64) -> bool {
    budget.saturating_sub(committed) < MIN_ROOM_MIB * PAGES_PER_MIB
}

/// fs's working state beside its block cache (kaifs's tables, the request
/// paths, its stack): what its quota held beside the 16 MiB cache it had alone.
pub const FS_WORK_MIB: u64 = 16;
/// The block caches fs may get, largest first (#1036). 64 MiB is ADR 0004's
/// (`kaifs::cache::DEFAULT_BLOCKS`); 16 MiB is what it had at a fixed size, on
/// any machine, and what a machine with no room for more still gets.
pub const FS_CACHES_MIB: [u64; 3] = [64, 32, 16];
/// The word on fs's plan line that names its cache, which fs reads.
pub const CACHE_WORD: &str = "cache_mib=";

/// What [`size_fs`] chose for fs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FsCache {
    pub cache_mib: u64,
    /// fs's quota: [`FS_WORK_MIB`] and the cache.
    pub pages: u64,
    /// What the plan's boot services, fs's new quota included, leave of the
    /// budget.
    pub room_mib: u64,
}

/// The largest of [`FS_CACHES_MIB`] whose quota still leaves [`MIN_ROOM_MIB`]
/// of `budget` and the plan's largest `start=demand` service (one app, cc or
/// fetch opened at a time) beside the other boot services; the smallest if
/// none does. None with no `fs` line, or one that names a cache its own
/// `pages=` holds. A named cache that does not fit its quota (fs would grow
/// past it and be killed) is sized as if it named none.
pub fn fs_cache(plan: &Plan, supervisor_pages: u64, budget: u64) -> Option<FsCache> {
    let fs = plan.services.iter().find(|s| s.program == "fs")?;
    if named_cache_fits(&fs.args, fs.limits.max_pages) {
        return None;
    }
    let others = boot_pages(plan, supervisor_pages).saturating_sub(match fs.start {
        Start::Boot | Start::Pkg => fs.limits.max_pages,
        Start::Demand => 0,
    });
    let margin = plan
        .services
        .iter()
        .filter(|s| s.start == Start::Demand)
        .map(|s| s.limits.max_pages)
        .max()
        .unwrap_or(0);
    let smallest = FS_CACHES_MIB[FS_CACHES_MIB.len() - 1];
    // Saturating, as `boot_pages` is: a line's `pages=` may be near u64::MAX.
    let cache_mib = FS_CACHES_MIB
        .into_iter()
        .find(|&cache| {
            others
                .saturating_add(fs_quota(cache))
                .saturating_add(MIN_ROOM_MIB * PAGES_PER_MIB)
                .saturating_add(margin)
                <= budget
        })
        .unwrap_or(smallest);
    let pages = fs_quota(cache_mib);
    Some(FsCache {
        cache_mib,
        pages,
        room_mib: budget.saturating_sub(others.saturating_add(pages)) / PAGES_PER_MIB,
    })
}

/// fs's quota with a cache of `cache_mib`: [`FS_WORK_MIB`] and the cache.
pub fn fs_quota(cache_mib: u64) -> u64 {
    FS_WORK_MIB.saturating_add(cache_mib) * PAGES_PER_MIB
}

/// Whether fs's `args` name a cache ([`CACHE_WORD`]) its `max_pages` holds,
/// as fs reads the word: a number clamped to 16..=64, and 16 for one that is
/// not a number.
fn named_cache_fits(args: &str, max_pages: u64) -> bool {
    let smallest = FS_CACHES_MIB[FS_CACHES_MIB.len() - 1];
    let Some(word) = args
        .split_whitespace()
        .find_map(|word| word.strip_prefix(CACHE_WORD))
    else {
        return false;
    };
    let cache = word
        .parse::<u64>()
        .unwrap_or(smallest)
        .clamp(smallest, FS_CACHES_MIB[0]);
    fs_quota(cache) <= max_pages
}

/// `plan` with fs's line given its [`fs_cache`]: `pages=` its quota, and
/// [`CACHE_WORD`] its cache, in place of any it named. Every other line is as
/// it was.
pub fn size_fs(plan: &str, supervisor_pages: u64, budget: u64) -> (String, Option<FsCache>) {
    let parsed = crate::parse_plan(plan);
    let Some(chosen) = fs_cache(&parsed, supervisor_pages, budget) else {
        return (String::from(plan), None);
    };
    let fs_name = parsed
        .services
        .iter()
        .find(|s| s.program == "fs")
        .map(|s| s.name.as_str())
        .unwrap_or("fs");
    let mut out = String::new();
    for line in plan.lines() {
        if !out.is_empty() {
            out.push('\n');
        }
        if line.split_whitespace().next() != Some(fs_name) {
            out.push_str(line);
            continue;
        }
        let mut words: alloc::vec::Vec<String> = line
            .split_whitespace()
            .filter(|w| !w.starts_with("pages=") && !w.starts_with(CACHE_WORD))
            .map(String::from)
            .collect();
        words.push(format!("pages={}", chosen.pages));
        words.push(format!("{CACHE_WORD}{}", chosen.cache_mib));
        out.push_str(&words.join(" "));
    }
    (out, Some(chosen))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_plan;
    use alloc::format;
    use alloc::string::String;

    #[test]
    fn counts_boot_and_pkg_services_and_the_supervisor_not_demand_ones() {
        let plan = parse_plan(
            "fs pages=8192\nshell pages=4096\nnotes pages=1024 start=demand\nextra pages=512 start=pkg",
        );
        assert!(plan.errors.is_empty(), "{:?}", plan.errors);
        assert_eq!(boot_pages(&plan, 2048), 2048 + 8192 + 4096 + 512);
    }

    #[test]
    fn a_line_without_pages_counts_the_default_quota() {
        let plan = parse_plan("llm");
        assert_eq!(boot_pages(&plan, 0), crate::DEFAULT_PAGES);
    }

    /// The system plan as the kernel builds it with `kai-data` and a model
    /// line of `llm_mib`, and the budget a machine of `ram_mib` gives it:
    /// half of what is free once the kernel and the boot modules are loaded.
    /// A 512 MiB VM has about 105 MiB of budget (desk-catalog, #877), so
    /// about 302 MiB goes before it.
    fn system(ram_mib: u64, llm_mib: u64) -> (String, u64) {
        let llm = format!("llm caps=serve pages={}", llm_mib * PAGES_PER_MIB);
        let plan = kabi::plan::system::plan(&llm, true);
        (plan, (ram_mib - 302) / 2 * PAGES_PER_MIB)
    }

    fn chosen(ram_mib: u64, llm_mib: u64) -> FsCache {
        let (plan, budget) = system(ram_mib, llm_mib);
        size_fs(&plan, 2048, budget).1.unwrap()
    }

    #[test]
    fn fs_gets_the_cache_its_machine_has_room_for() {
        // 512 MiB: no room beyond today's 16 MiB cache and 32 MiB quota.
        let small = chosen(512, 8);
        assert_eq!((small.cache_mib, small.pages), (16, 8192), "{small:?}");
        // 1, 2 and 4 GiB: ADR 0004's 64 MiB.
        for ram in [1024, 2048, 4096] {
            let big = chosen(ram, 8);
            assert_eq!((big.cache_mib, big.pages), (64, 80 * 256), "{ram}: {big:?}");
        }
        // 1 GiB beside a model that takes most of it: the middle step.
        let tight_1g = chosen(1024, 160);
        assert_eq!(tight_1g.cache_mib, 32, "{tight_1g:?}");
    }

    #[test]
    fn the_chosen_cache_leaves_min_room_and_the_largest_demand_service() {
        for ram in [512, 768, 1024, 2048, 4096] {
            for llm in [8, 64, 128, 256] {
                let (plan, budget) = system(ram, llm);
                let (sized, fs) = size_fs(&plan, 2048, budget);
                let fs = fs.unwrap();
                let parsed = crate::parse_plan(&sized);
                assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
                let line = parsed.services.iter().find(|s| s.name == "fs").unwrap();
                assert_eq!(line.limits.max_pages, fs.pages);
                assert!(line.args.contains(&format!("cache_mib={}", fs.cache_mib)));
                if fs.cache_mib > 16 {
                    let committed = boot_pages(&parsed, 2048);
                    let margin = parsed
                        .services
                        .iter()
                        .filter(|s| s.start == Start::Demand)
                        .map(|s| s.limits.max_pages)
                        .max()
                        .unwrap();
                    assert!(!tight(committed + margin, budget), "{ram} {llm}: {fs:?}");
                }
            }
        }
    }

    #[test]
    fn other_lines_and_a_named_cache_are_left_alone() {
        let plan = "fs caps=serve+partition pages=8192 stack_pages=64\nshell pages=4096";
        let (sized, fs) = size_fs(plan, 2048, 1 << 20);
        assert_eq!(fs.unwrap().cache_mib, 64);
        assert_eq!(
            sized,
            "fs caps=serve+partition stack_pages=64 pages=20480 cache_mib=64\nshell pages=4096"
        );
        let pinned = "fs pages=8192 cache_mib=16\nshell pages=4096";
        assert_eq!(size_fs(pinned, 2048, 1 << 20), (String::from(pinned), None));
        assert_eq!(
            size_fs("shell", 2048, 1 << 20),
            (String::from("shell"), None)
        );
    }

    #[test]
    fn a_named_cache_its_quota_cannot_hold_is_sized_again() {
        // 64 MiB of cache in a 32 MiB quota: fs would outgrow it and be killed.
        let over = "fs pages=8192 cache_mib=64\nshell pages=4096";
        let (sized, fs) = size_fs(over, 2048, 1 << 20);
        assert_eq!(fs.unwrap().cache_mib, 64);
        assert_eq!(sized, "fs pages=20480 cache_mib=64\nshell pages=4096");
        // On a budget with no room for more, the smallest, in its own quota.
        let (sized, fs) = size_fs(over, 2048, 16 * PAGES_PER_MIB);
        assert_eq!(fs.unwrap().cache_mib, 16);
        assert_eq!(sized, "fs pages=8192 cache_mib=16\nshell pages=4096");
        // A word fs reads as 16 fits a 32 MiB quota, and is left alone.
        let odd = "fs pages=8192 cache_mib=lots";
        assert_eq!(size_fs(odd, 2048, 1 << 20), (String::from(odd), None));
        // One that fits is left alone, as before.
        let fits = "fs pages=20480 cache_mib=64";
        assert_eq!(size_fs(fits, 2048, 1 << 20), (String::from(fits), None));
    }

    #[test]
    fn a_huge_pages_line_saturates_and_never_overflows() {
        let plan = format!("fs pages=8192\nshell pages={}", u64::MAX);
        let (_, fs) = size_fs(&plan, 2048, 1 << 20);
        let fs = fs.unwrap();
        assert_eq!((fs.cache_mib, fs.room_mib), (16, 0), "{fs:?}");
    }

    /// The margin is one demand service, as #1036 proposed. On a 1 GiB machine
    /// with no model and the full system plan, the boot services still admit
    /// with MIN_ROOM and the largest demand service beside them. Every demand
    /// service open at once (296 MiB) fits neither here nor on main (its 16 MiB
    /// cache leaves 249 MiB), so a margin of all of them would allow no cache
    /// above 16 MiB on 1 GiB. The 64 MiB cache costs 48 MiB of that room.
    #[test]
    fn a_1_gib_machine_with_no_model_still_admits_the_full_plan() {
        let (plan, budget) = system(1024, 8);
        let (sized, fs) = size_fs(&plan, 2048, budget);
        let fs = fs.unwrap();
        assert_eq!((fs.cache_mib, fs.room_mib), (64, 201), "{fs:?}");
        let parsed = crate::parse_plan(&sized);
        let committed = boot_pages(&parsed, 2048);
        assert!(committed <= budget);
        let demand = parsed
            .services
            .iter()
            .filter(|s| s.start == Start::Demand)
            .map(|s| s.limits.max_pages);
        let (largest, all) = (demand.clone().max().unwrap(), demand.sum::<u64>());
        assert!(!tight(committed + largest, budget));
        assert_eq!(all / PAGES_PER_MIB, 296);
        let main_room = fs.room_mib + (fs.cache_mib - 16);
        assert!(
            all / PAGES_PER_MIB > main_room,
            "not even main fit them all"
        );
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
