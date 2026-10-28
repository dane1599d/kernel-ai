//! Anonymous memory (lane M): MEM_RESERVE, MEM_COMMIT and MEM_RELEASE. None
//! needs a capability: each works only in the caller's own address space, and a
//! commit counts against its own page quota.

use crate::process::mem;
use kabi::{Error, call};

/// The calls this area serves.
const CALLS: &[u64] = &[call::MEM_RESERVE, call::MEM_COMMIT, call::MEM_RELEASE];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| match number {
        call::MEM_RESERVE => mem::reserve(slot, a[0]),
        call::MEM_COMMIT => mem::commit(slot, a[0], a[1]),
        _ => mem::release(slot, a[0]),
    })
}
