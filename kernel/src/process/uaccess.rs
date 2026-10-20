//! Copying between the kernel and a process's memory. The kernel never dereferences
//! a user pointer: each page is looked up in the process's own page tables (it must
//! be a user page, and writable for writes) and copied through the direct map. Only
//! the process's own threads call these, inside their own system calls, and a
//! process's pages are never unmapped while any of its threads lives, so they cannot
//! go away while a copy runs.

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
