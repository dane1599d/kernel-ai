//! Devices (lane D, ADR 0007): DEV_LIST.

use crate::process::uaccess;
use kabi::device::{DeviceRecord, MAX_RECORDS};
use kabi::{Error, call};

/// The calls this area serves.
const CALLS: &[u64] = &[call::DEV_LIST];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| dev_list(slot, a[0], a[1]))
}

/// Copies as many whole records as fit in `cap` bytes at `buf`.
fn dev_list(slot: usize, buf: u64, cap: u64) -> Result<u64, Error> {
    let mut records = [DeviceRecord::default(); MAX_RECORDS];
    let fits = usize::try_from(cap / core::mem::size_of::<DeviceRecord>() as u64)
        .unwrap_or(MAX_RECORDS)
        .min(MAX_RECORDS);
    let count = crate::devices::list(&mut records[..fits]);
    // SAFETY: DeviceRecord is plain old data with no padding the kernel did
    // not write (its reserved fields are zero).
    let bytes = unsafe {
        core::slice::from_raw_parts(
            records.as_ptr().cast::<u8>(),
            count * core::mem::size_of::<DeviceRecord>(),
        )
    };
    uaccess::copy_to_user(slot, buf, bytes)?;
    Ok(bytes.len() as u64)
}
