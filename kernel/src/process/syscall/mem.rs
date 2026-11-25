//! Anonymous memory (lane M): MEM_RESERVE, MEM_COMMIT and MEM_RELEASE, which
//! need no capability (each works only in the caller's own address space, and a
//! commit counts against its own page quota); and resource profiles'
//! PROC_QUOTA_SET and PROC_CLASS_SET (P3 task 12), which need SUPERVISE; and
//! PROC_ALIVE, which needs DISPLAY or SUPERVISE.

use super::need;
use crate::process::{mem, profile};
use kabi::{Error, call, cap};

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::MEM_RESERVE,
    call::MEM_COMMIT,
    call::MEM_RELEASE,
    call::MEM_EXEC,
    call::PROC_QUOTA_SET,
    call::PROC_CLASS_SET,
    call::PROC_ALIVE,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| match number {
        call::MEM_RESERVE => mem::reserve(slot, a[0]),
        call::MEM_COMMIT => mem::commit(slot, a[0], a[1]),
        call::MEM_RELEASE => mem::release(slot, a[0]),
        call::MEM_EXEC => mem::exec(slot, a[0], a[1]),
        call::PROC_QUOTA_SET => {
            need(slot, cap::SUPERVISE).and_then(|()| profile::quota_set_with(a[0], a[1], a[2]))
        }
        call::PROC_ALIVE => need(slot, cap::DISPLAY)
            .or_else(|_| need(slot, cap::SUPERVISE))
            .map(|()| u64::from(crate::process::alive(a[0]))),
        _ => need(slot, cap::SUPERVISE).and_then(|()| profile::class_set(a[0], a[1])),
    })
}
