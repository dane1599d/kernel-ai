//! MEM_RESERVE, MEM_COMMIT and MEM_RELEASE (lane M, for P3 task 11's suspend):
//! anonymous memory a process takes and gives back, so a suspended app's pages
//! return to the system (the founder: "don't give it any memory until
//! reopened"). The book is `mm::reserve`, kept in the address space under its
//! lock.
//! - A commit maps fresh zeroed pages, counted against the page quota as the
//!   heap's are; past the quota it fails with NoMemory and leaves the process
//!   running, since a wasm `memory.grow` may fail.
//! - A release takes the pages out of the page tables under the space's lock,
//!   shoots the range down on every CPU with no lock held, and only then frees
//!   the frames. No stale TLB entry reaches a reused frame, and a process that
//!   exits in between can neither free them twice (they are no longer in its
//!   tables) nor leak them (the release frees them).

use super::{lock_space, with_table};
use crate::memory;
use crate::memory::user::Access;
use crate::smp;
use alloc::vec::Vec;
use kabi::Error;
use mm::PAGE_SIZE;
use mm::reserve::Refused;

fn refused(why: Refused) -> Error {
    match why {
        Refused::BadSize => Error::Invalid,
        Refused::Full => Error::NoMemory,
        Refused::NotFound | Refused::Releasing => Error::NotFound,
        Refused::Hole => Error::Invalid,
    }
}

fn pages_of(bytes: u64) -> Result<u64, Error> {
    match bytes.div_ceil(PAGE_SIZE) {
        0 => Err(Error::Invalid),
        pages => Ok(pages),
    }
}

/// MEM_RESERVE: `bytes` (whole pages) of address space, nothing mapped; its start.
pub fn reserve(slot: usize, bytes: u64) -> Result<u64, Error> {
    let pages = pages_of(bytes)?;
    let mut guard = lock_space(slot);
    let space = guard.as_mut().ok_or(Error::Dead)?;
    space.reservations.reserve(pages).map_err(refused)
}

/// MEM_COMMIT: maps `[addr, addr + bytes)` of a reservation, from what is
/// committed already; pages committed before stay as they are.
pub fn commit(slot: usize, addr: u64, bytes: u64) -> Result<u64, Error> {
    let pages = pages_of(bytes)?;
    let mut guard = lock_space(slot);
    let space = guard.as_mut().ok_or(Error::Dead)?;
    let max_pages = with_table(|table| table[slot].limits.max_pages);
    let plan = space
        .reservations
        .plan_commit(addr, pages)
        .map_err(refused)?;
    let data = Access {
        write: true,
        execute: false,
    };
    let mut mapped = 0;
    let mut result = Ok(0);
    while mapped < plan.pages {
        if space.frames() >= max_pages {
            result = Err(Error::NoMemory);
            break;
        }
        let at = plan.at + mapped * PAGE_SIZE;
        if memory::with(|m| m.map_user_page(space, at, data)).is_err() {
            result = Err(Error::NoMemory);
            break;
        }
        mapped += 1;
    }
    space.reservations.committed(addr, mapped);
    result
}

/// MEM_EXEC (S14-2, spike): the committed pages of `[addr, addr + bytes)`, which
/// must lie in the process's reservation area, become read-execute.
pub fn exec(slot: usize, addr: u64, bytes: u64) -> Result<u64, Error> {
    let pages = pages_of(bytes)?;
    if !addr.is_multiple_of(PAGE_SIZE)
        || addr < kabi::layout::MEM_BASE
        || addr.saturating_add(pages * PAGE_SIZE) > kabi::layout::MEM_END
    {
        return Err(Error::Invalid);
    }
    let (changed, cr3) = {
        let mut guard = lock_space(slot);
        let space = guard.as_mut().ok_or(Error::Dead)?;
        let changed = memory::with(|m| m.protect_exec(space, addr, pages));
        (changed, space.cr3())
    };
    crate::smp::tlb_shootdown_range(&[cr3], addr, pages);
    Ok(changed)
}

/// MEM_RELEASE: unmaps and frees every page of the reservation starting at
/// `addr`, which then ends; how many pages were freed. It walks one page past
/// the committed ones ([`mm::reserve::Release::walk`]), so the tables a commit
/// cut short left there (a directory at a 1 GiB boundary with no leaf under
/// it) go too, after the same shootdown, which therefore never covers 0 pages.
pub fn release(slot: usize, addr: u64) -> Result<u64, Error> {
    let pages = {
        let mut guard = lock_space(slot);
        let space = guard.as_mut().ok_or(Error::Dead)?;
        space
            .reservations
            .begin_release(addr)
            .map_err(refused)?
            .walk
    };
    // Room for every frame before any lock: none is taken while allocating.
    // Should this fail, the pages stay mapped until the process ends, which
    // frees them.
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(crate::memory::user::release_frames(pages) as usize)
        .map_err(|_| Error::NoMemory)?;
    // Preemption stays off from the unmap through the shootdown: moved to
    // another CPU between the local flush and the choice of whom to send it
    // to, this thread would skip a CPU still holding the range (audit K #27).
    crate::threads::preempt_disable();
    let taken = {
        let mut guard = lock_space(slot);
        guard.as_mut().map(|space| {
            let tables = memory::with(|m| m.take_owned_range(space, addr, pages, &mut frames));
            (space.cr3(), tables)
        })
    };
    if let Some((cr3, _)) = taken {
        smp::tlb_shootdown_range(&[cr3], addr, pages);
    }
    crate::threads::preempt_enable();
    let (_, tables) = taken.ok_or(Error::Dead)?;
    memory::with(|m| m.free_user_frames(&frames));
    if let Some(space) = lock_space(slot).as_mut() {
        space.reservations.end_release(addr);
    }
    // The committed pages it gave back; the tables freed with them are not
    // the reservation's.
    Ok(frames.len() as u64 - tables)
}
