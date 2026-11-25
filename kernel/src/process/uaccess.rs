//! Copying between the kernel and a process's memory. The kernel never dereferences
//! a user pointer: each page is looked up in the process's own page tables (it must
//! be a user page, and writable for writes) and copied through the direct map. Only
//! the process's own threads call these, inside their own system calls. A page can
//! be unmapped while the process lives (MEM_RELEASE, SHM_UNMAP), but only under its
//! space lock, and its frame is freed only after: so each access here holds that
//! lock from the lookup through the copy, and a frame is never touched after it.

use super::with_space;
use crate::memory;
use crate::memory::user::MIN_USER_ADDRESS;
use kabi::Error;
use kabi::layout::USER_END;
use mm::PAGE_SIZE;
use x86_64::structures::paging::PageTableFlags;

/// Calls `f(physical address, offset into the buffer, length)` for each page piece
/// of `[addr, addr + len)`, after checking the whole range first.
fn each_page(
    slot: usize,
    addr: u64,
    len: usize,
    write: bool,
    mut f: impl FnMut(u64, usize, usize),
) -> Result<(), Error> {
    if len == 0 {
        return Ok(());
    }
    let end = addr.checked_add(len as u64).ok_or(Error::Fault)?;
    if addr < MIN_USER_ADDRESS || end > USER_END {
        return Err(Error::Fault);
    }
    let mut needed = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if write {
        needed |= PageTableFlags::WRITABLE;
    }
    // Pieces of the range, one per page: (address, length).
    let pieces = move || {
        let mut at = addr;
        core::iter::from_fn(move || {
            (at < end).then(|| {
                let piece = (at, (end - at).min(PAGE_SIZE - at % PAGE_SIZE));
                at += piece.1;
                piece
            })
        })
    };
    // The space stays locked across both passes, so the check holds for the copy.
    with_space(slot, |space| {
        let page = |at: u64| {
            memory::with(|m| m.translate_user(space, at))
                .filter(|(_, flags)| flags.contains(needed))
                .map(|(phys, _)| phys)
                .ok_or(Error::Fault)
        };
        for (at, _) in pieces() {
            page(at)?;
        }
        for (at, chunk) in pieces() {
            f(page(at)?, (at - addr) as usize, chunk as usize);
        }
        Ok(())
    })
    .unwrap_or(Err(Error::Fault))
}

/// Runs `f` with the physical address of user address `addr` in the process,
/// which must be mapped for ring 3 (Fault otherwise), while the process's space
/// stays locked: MEM_RELEASE and SHM_UNMAP unmap a page only under that lock and
/// free its frame after, so the frame is the process's for as long as `f` runs,
/// and no longer. `f` runs with preemption off and must not take SPACES or sleep.
pub fn with_phys<R>(slot: usize, addr: u64, f: impl FnOnce(u64) -> R) -> Result<R, Error> {
    if !(MIN_USER_ADDRESS..USER_END).contains(&addr) {
        return Err(Error::Fault);
    }
    let needed = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    with_space(slot, |space| {
        memory::with(|m| m.translate_user(space, addr))
            .filter(|(_, flags)| flags.contains(needed))
            .map(|(phys, _)| f(phys))
    })
    .flatten()
    .ok_or(Error::Fault)
}

/// Copies `out.len()` bytes from user address `addr`.
pub fn copy_from_user(slot: usize, addr: u64, out: &mut [u8]) -> Result<(), Error> {
    let hhdm = memory::hhdm();
    let len = out.len();
    let target = out.as_mut_ptr();
    each_page(slot, addr, len, false, |phys, offset, chunk| {
        // SAFETY: `phys` is a user frame of this process, mapped by the HHDM, and
        // `offset + chunk <= len`.
        unsafe {
            core::ptr::copy_nonoverlapping((hhdm + phys) as *const u8, target.add(offset), chunk)
        };
    })
}

/// Copies `data` to user address `addr`, which must be writable.
pub fn copy_to_user(slot: usize, addr: u64, data: &[u8]) -> Result<(), Error> {
    let hhdm = memory::hhdm();
    each_page(slot, addr, data.len(), true, |phys, offset, chunk| {
        // SAFETY: as above, in the other direction.
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr().add(offset),
                (hhdm + phys) as *mut u8,
                chunk,
            )
        };
    })
}

/// Reads a `#[repr(C)]` structure of plain `u64` fields from user memory.
pub fn read_struct<T: Copy + Default>(slot: usize, addr: u64) -> Result<T, Error> {
    let mut value = T::default();
    // SAFETY: T is plain old data (only u64 fields in kabi), so any bytes are valid.
    let bytes = unsafe {
        core::slice::from_raw_parts_mut((&raw mut value).cast::<u8>(), core::mem::size_of::<T>())
    };
    copy_from_user(slot, addr, bytes)?;
    Ok(value)
}
