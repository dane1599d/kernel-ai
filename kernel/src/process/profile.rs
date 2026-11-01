//! Resource profiles' kernel half (P3 task 12): a process's page quota
//! (PROC_QUOTA_SET), which the heap, `Shm` and MEM_COMMIT all charge against,
//! and its CPU class on M10's `classes` policy (PROC_CLASS_SET). Both need
//! SUPERVISE. The floors are the kernel's, whatever the shell asked (P3 plan,
//! "Resource profiles' floors"):
//! - what the user needs to stay in control, the shell, the supervisor, the
//!   model service (its pages and KV cache) and `desktop`, which holds the
//!   trusted layer, are never made `background`, and their quota is never set
//!   below what they have;
//! - no quota is raised past the global admission rule: every live process's
//!   quota adds up to at most the budget (free / 2 at boot), so the kernel's
//!   own half of memory is never promised away;
//! - a system service, one holding a capability of the running system
//!   ([`SYSTEM_CAPS`]: the terminal's TERM, `fs`'s PARTITION and the like), is
//!   never made `background`, so it keeps its CPU share under every profile.

use crate::sched_control;
use kabi::{Error, cap};

/// Capabilities that make a process part of the running system rather than an
/// app: apps (`comphost`) and the user's own programs hold none of them.
pub const SYSTEM_CAPS: u64 = cap::SUPERVISE
    | cap::NET
    | cap::POLICY
    | cap::MODEL
    | cap::PARTITION
    | cap::SYSTEM_STATE
    | cap::DISPLAY
    | cap::INPUT
    | cap::TERM
    | cap::UPDATE;

/// The quota a profile may not go below for anyone: enough for a program's
/// image, stack and a little heap.
pub const MIN_PAGES: u64 = 64;

/// The services no profile may slow or shrink.
const FLOORED: &[&str] = &["shell", "supervisor", "llm", super::supervision::DESKTOP];

fn floored(name: &str) -> bool {
    FLOORED.contains(&name)
}

/// PROC_QUOTA_SET: process `pid`'s page quota becomes `pages`; the old one.
/// Pages it holds past a lower quota stay: its next allocation fails instead.
pub fn quota_set(pid: u64, pages: u64) -> Result<u64, Error> {
    if pages < MIN_PAGES {
        return Err(Error::Invalid);
    }
    super::with_table(|table| {
        let at = table
            .iter()
            .position(|p| p.live() && p.pid == pid && pid != 0)
            .ok_or(Error::NotFound)?;
        let old = table[at].limits.max_pages;
        if floored(table[at].name.as_str()) && pages < old {
            return Err(Error::Permission);
        }
        if pages > old {
            let committed: u64 = table
                .iter()
                .filter(|p| p.live())
                .map(|p| p.limits.max_pages)
                .sum();
            // The others' quotas include none of this one's, and the
            // comparison never adds `pages`, so no size of it can overflow.
            let others = committed.saturating_sub(old);
            if pages > super::page_budget().saturating_sub(others) {
                return Err(Error::NoMemory);
            }
        }
        table[at].limits.max_pages = pages;
        Ok(old)
    })
}

/// PROC_CLASS_SET: process `pid` runs `normal` (0) or `background` (1) from
/// now on, every thread on every CPU. Busy while a tuning trial runs: the trial
/// owns the labels until it ends.
pub fn class_set(pid: u64, class: u64) -> Result<u64, Error> {
    let background = match class {
        0 => false,
        1 => true,
        _ => return Err(Error::Invalid),
    };
    let (name, caps) = super::with_table(|table| {
        table
            .iter()
            .find(|p| p.live() && p.pid == pid && p.state != super::State::Exiting)
            .map(|p| (p.name, p.limits.caps))
    })
    .ok_or(Error::NotFound)?;
    if background && (floored(name.as_str()) || caps & SYSTEM_CAPS != 0) {
        return Err(Error::Permission);
    }
    sched_control::label(pid, background)?;
    Ok(0)
}
