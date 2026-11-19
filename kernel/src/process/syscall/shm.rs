//! Shared pages (ADR 0001 D4): SHM_CREATE, SHM_MAP, SHM_UNMAP, SHM_INFO and
//! SHM_SIZE. None needs a capability: SHM_MAP, SHM_INFO and SHM_SIZE need the
//! object's handle, and SHM_CREATE counts against the caller's own page quota.

use crate::shm;
use kabi::{Error, call};

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::SHM_CREATE,
    call::SHM_MAP,
    call::SHM_UNMAP,
    call::SHM_INFO,
    call::SHM_SIZE,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| match number {
        call::SHM_CREATE => shm::create(slot, a[0]),
        call::SHM_MAP => match a[1] {
            0 => shm::map(slot, a[0], false, a[2]),
            1 => shm::map(slot, a[0], true, a[2]),
            _ => Err(Error::Invalid),
        },
        call::SHM_INFO => shm::info(slot, a[0]),
        call::SHM_SIZE => shm::size(slot, a[0]),
        _ => shm::unmap(slot, a[0]),
    })
}
