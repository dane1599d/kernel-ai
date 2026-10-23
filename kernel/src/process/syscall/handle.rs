//! Handles (ADR 0001 D1, D10): the ABI version, the startup handles, and reading,
//! narrowing and closing a handle. None needs a capability: a process only ever
//! acts on its own table.

use crate::handle::{self, Handles};
use crate::process::uaccess;
use alloc::vec::Vec;
use kabi::handle::{ABI, AbiInfo, Handle, HandleInfo, MAX_STARTUP, Rights, TABLE_MAX};
use kabi::{Error, call};

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::ABI_INFO,
    call::STARTUP,
    call::HANDLE_INFO,
    call::HANDLE_DUP,
    call::HANDLE_REPLACE,
    call::HANDLE_CLOSE,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| run(slot, number, a))
}

fn run(slot: usize, number: u64, a: &[u64; 6]) -> Result<u64, Error> {
    match number {
        call::ABI_INFO => {
            let info = AbiInfo {
                major: ABI.0,
                minor: ABI.1,
                max_call: call::MAX as u32,
                table_max: TABLE_MAX as u32,
            };
            copy_out(slot, a[0], &[info])?;
            Ok(0)
        }
        call::STARTUP => startup(slot, a[0], a[1]),
        call::HANDLE_INFO => {
            let handle = handle_arg(a[0])?;
            let info = handle::with(slot, |handles| {
                handles
                    .table
                    .get(handle)
                    .map(|entry| handle::info(handle, entry))
            })?
            .map_err(Error::from)?;
            copy_out(slot, a[1], &[info])?;
            Ok(0)
        }
        call::HANDLE_DUP => {
            let (handle, rights) = (handle_arg(a[0])?, rights_arg(a[1])?);
            let new = handle::with(slot, |handles| handles.table.dup(handle, rights))?;
            Ok(u64::from(new.map_err(Error::from)?.0))
        }
        call::HANDLE_REPLACE => {
            let (handle, rights) = (handle_arg(a[0])?, rights_arg(a[1]));
            let new = handle::with(slot, |handles| match rights {
                Ok(rights) => handles.table.replace(handle, rights),
                // Closed either way, as a refused narrowing would close it.
                Err(_) => handles
                    .table
                    .close(handle)
                    .and(Err(captable::Error::Invalid)),
            })?;
            Ok(u64::from(new.map_err(Error::from)?.0))
        }
        call::HANDLE_CLOSE => {
            let handle = handle_arg(a[0])?;
            handle::with(slot, |handles| handles.table.close(handle))?.map_err(Error::from)?;
            Ok(0)
        }
        _ => Err(Error::NotFound),
    }
}

/// A handle argument: 32 bits, never 0.
fn handle_arg(raw: u64) -> Result<Handle, Error> {
    match u32::try_from(raw) {
        Ok(0) | Err(_) => Err(Error::BadHandle),
        Ok(value) => Ok(Handle(value)),
    }
}

/// A rights argument: only defined bits.
fn rights_arg(raw: u64) -> Result<Rights, Error> {
    u32::try_from(raw)
        .ok()
        .and_then(Rights::from_bits)
        .ok_or(Error::Invalid)
}

/// STARTUP: the startup handles still open, up to `cap` of them to `out`; how
/// many there are.
fn startup(slot: usize, out: u64, cap: u64) -> Result<u64, Error> {
    let list: Vec<HandleInfo> = handle::with(slot, |handles: &mut Handles| {
        handles.startup().take(MAX_STARTUP).collect()
    })?;
    let fits = usize::try_from(cap).unwrap_or(usize::MAX).min(list.len());
    copy_out(slot, out, &list[..fits])?;
    Ok(list.len() as u64)
}

/// Copies plain structs to ring 3.
fn copy_out<T: Copy>(slot: usize, addr: u64, items: &[T]) -> Result<(), Error> {
    // SAFETY: `T` is one of the `repr(C)` structs above, plain old data with no
    // padding, so its bytes are all initialised.
    let bytes = unsafe {
        core::slice::from_raw_parts(items.as_ptr().cast::<u8>(), core::mem::size_of_val(items))
    };
    uaccess::copy_to_user(slot, addr, bytes)
}
