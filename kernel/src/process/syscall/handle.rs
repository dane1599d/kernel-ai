//! Handles (ADR 0001 D1, D5, D10): the ABI version, the startup handles,
//! reading, narrowing and closing a handle, and grant nodes. None needs a
//! capability: a process only ever acts on its own table, and making or
//! revoking a node needs a `Grant` handle with MINT or REVOKE.

use crate::handle::grant::{self as grants, Kernel};
use crate::handle::{self, Handles};
use crate::process::uaccess;
use alloc::vec::Vec;
use captable::grant::{GrantRef, Life};
use kabi::handle::{ABI, AbiInfo, Handle, HandleInfo, Kind, MAX_STARTUP, Rights, TABLE_MAX};
use kabi::{Error, call};

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::ABI_INFO,
    call::STARTUP,
    call::HANDLE_INFO,
    call::HANDLE_DUP,
    call::HANDLE_REPLACE,
    call::HANDLE_CLOSE,
    call::GRANT_NEW,
    call::GRANT_REVOKE,
    call::GRANT_DUP,
    call::ONCE_CHECK,
    call::RULE_CHECK,
    call::ONCE_CHECK_FOR,
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
            let new = handle::with(slot, |handles| handles.table.dup(handle, rights, &Kernel))?;
            Ok(u64::from(new.map_err(Error::from)?.0))
        }
        call::HANDLE_REPLACE => {
            let (handle, rights) = (handle_arg(a[0])?, rights_arg(a[1]));
            let new = handle::with(slot, |handles| match rights {
                Ok(rights) => handles.table.replace(handle, rights, &Kernel),
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
        call::GRANT_NEW => grant_new(slot, a[0], a[1], a[2], a[3]),
        call::GRANT_REVOKE => {
            let node = grant_arg(slot, a[0], Rights::REVOKE)?;
            grants::revoke(node).map(|count| count as u64)
        }
        call::GRANT_DUP => {
            let (handle, rights) = (handle_arg(a[0])?, rights_arg(a[1])?);
            let node = grant_arg(slot, a[2], Rights::MINT)?;
            let new = handle::with(slot, |h| h.table.dup_under(handle, rights, node, &Kernel))?;
            Ok(u64::from(new.map_err(Error::from)?.0))
        }
        call::ONCE_CHECK => {
            let mut digest = [0u8; 32];
            uaccess::copy_from_user(slot, a[1], &mut digest)?;
            grants::spend_once(a[0], &digest).map(|()| 0)
        }
        call::RULE_CHECK => rule_check(slot, a[0], a[1], a[2]),
        call::ONCE_CHECK_FOR => {
            super::need(slot, kabi::cap::SUPERVISE)?;
            let mut digest = [0u8; 32];
            uaccess::copy_from_user(slot, a[2], &mut digest)?;
            grants::spend_once_for(a[0], a[1], &digest).map(|()| 0)
        }
        _ => Err(Error::NotFound),
    }
}

/// The longest call text RULE_CHECK reads.
const MAX_CALL_TEXT: usize = 256;

/// RULE_CHECK: whether the process `pid` holds a rule covering the call at
/// `ptr`. For the supervisor, which runs service calls for the shell.
fn rule_check(slot: usize, pid: u64, ptr: u64, len: u64) -> Result<u64, Error> {
    super::need(slot, kabi::cap::SUPERVISE)?;
    let len = usize::try_from(len).map_err(|_| Error::TooBig)?;
    if len == 0 || len > MAX_CALL_TEXT {
        return Err(Error::TooBig);
    }
    let mut text = [0u8; MAX_CALL_TEXT];
    uaccess::copy_from_user(slot, ptr, &mut text[..len])?;
    let call = core::str::from_utf8(&text[..len]).map_err(|_| Error::Invalid)?;
    let holder =
        crate::process::with_table(|table| table.iter().position(|p| p.live() && p.pid == pid))
            .filter(|_| pid != 0)
            .ok_or(Error::NotFound)?;
    match handle::rules::covers(holder, call) {
        true => Ok(0),
        false => Err(Error::Permission),
    }
}

/// The node a `Grant` handle names, if it carries `right` and is alive.
fn grant_arg(slot: usize, raw: u64, right: Rights) -> Result<GrantRef, Error> {
    let handle = handle_arg(raw)?;
    let entry = handle::with(slot, |h| h.table.check(handle, Kind::Grant, right, &Kernel))?
        .map_err(Error::from)?;
    // `check` refused it if its node was revoked; a root's handle has none.
    Ok(GrantRef::unpack(entry.object))
}

/// GRANT_NEW: a node under the one `parent` names, and a `Grant` handle to it.
fn grant_new(
    slot: usize,
    parent: u64,
    life: u64,
    label_ptr: u64,
    label_len: u64,
) -> Result<u64, Error> {
    let parent = grant_arg(slot, parent, Rights::MINT)?;
    let life = match life {
        0 => Life::Request,
        1 => Life::Session,
        2 => Life::Persistent,
        _ => return Err(Error::Invalid),
    };
    let len = usize::try_from(label_len).map_err(|_| Error::TooBig)?;
    if len > captable::grant::LABEL {
        return Err(Error::TooBig);
    }
    let mut label = [0u8; captable::grant::LABEL];
    uaccess::copy_from_user(slot, label_ptr, &mut label[..len])?;
    let owner = crate::process::identity(slot).pid;
    let node = grants::create(parent, owner, life, &label[..len])?;
    let rights =
        Rights::MINT | Rights::REVOKE | Rights::INSPECT | Rights::DUPLICATE | Rights::TRANSFER;
    let entry = captable::Entry {
        grant: Some(node),
        ..captable::Entry::new(Kind::Grant, rights, node.pack())
    };
    match grants::insert(slot, entry) {
        Ok(handle) => Ok(u64::from(handle.0)),
        Err(error) => {
            // No handle names it, so nothing could ever revoke it.
            let _ = grants::revoke(node);
            Err(error)
        }
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
