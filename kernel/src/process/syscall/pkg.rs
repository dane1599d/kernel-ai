//! Packages (lane P): BOOT_READ, the image's signed repository for `pkg`
//! (P4 task 1, ADR 0011 §8).

use crate::process::uaccess::{copy_from_user, copy_to_user};
use kabi::{Error, call};
use supervise::MAX_BOOT_PATH;

/// The calls this area serves.
const CALLS: &[u64] = &[call::BOOT_READ];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS
        .contains(&number)
        .then(|| boot_read(slot, a[0], a[1], a[2], a[3]))
}

/// `boot_read(path_ptr, path_len, buf, cap) -> len`: only beneath the folder the
/// caller's plan line grants with `boot=` (set at spawn from the kernel's own
/// plan, for that line's own program), and only a plain path there
/// (`supervise::BootFiles::allows`), looked up exactly.
fn boot_read(slot: usize, path_ptr: u64, path_len: u64, buf: u64, cap: u64) -> Result<u64, Error> {
    let files = crate::process::identity(slot)
        .boot
        .ok_or(Error::Permission)?;
    let len = usize::try_from(path_len).map_err(|_| Error::TooBig)?;
    if len > MAX_BOOT_PATH {
        return Err(Error::TooBig);
    }
    let mut path = [0u8; MAX_BOOT_PATH];
    copy_from_user(slot, path_ptr, &mut path[..len])?;
    let path = core::str::from_utf8(&path[..len]).map_err(|_| Error::Invalid)?;
    if !files.allows(path) {
        return Err(Error::Permission);
    }
    let data = crate::bootfiles::exact(path).ok_or(Error::NotFound)?;
    if cap >= data.len() as u64 {
        copy_to_user(slot, buf, data)?;
    }
    Ok(data.len() as u64)
}
