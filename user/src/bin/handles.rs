//! Test program for handles (`test=handles`, ADR 0001 steps 1-3). Started with
//! only the POLICY capability, it checks from ring 3 that:
//!
//! - ABI_INFO names ABI 1.0, the highest call and the table's size;
//! - STARTUP lists one `Sched` handle (READ, SET_POLICY, DUPLICATE, INSPECT), the
//!   one POLICY gives, and HANDLE_INFO reads the same;
//! - a duplicate gets a subset of the rights or fails `Invalid`, and one without
//!   DUPLICATE cannot be duplicated again; a forged handle is `BadHandle`;
//! - SCHED_STATS, which needs POLICY, works while the `Sched` handle with
//!   SET_POLICY is open, and fails `Permission` once it is closed, even though a
//!   narrower READ-only duplicate is still open: closing drops the right for good;
//! - a closed handle is `BadHandle`, and STARTUP no longer lists it.
//!
//! It prints `handles: every check passed` and exits 0, or names the first check
//! that failed and exits 1.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use kabi::Error;
use kabi::handle::{ABI, Handle, HandleInfo, Kind, Rights, TABLE_MAX};
use kabi::sched::SchedStats;
use user::{println, sys};

user::entry!(main);

fn main() -> i32 {
    match checks() {
        Ok(()) => {
            println!("handles: every check passed");
            0
        }
        Err(failed) => {
            println!("handles: FAILED: {failed}");
            1
        }
    }
}

fn expect<T: core::fmt::Debug>(
    got: Result<T, Error>,
    want: Error,
    what: &'static str,
) -> Result<(), &'static str> {
    match got {
        Err(error) if error == want => Ok(()),
        other => {
            println!("handles: {what}: got {other:?}, wanted {want:?}");
            Err(what)
        }
    }
}

fn checks() -> Result<(), &'static str> {
    let abi = sys::abi_info().map_err(|_| "abi_info failed")?;
    if (abi.major, abi.minor) != ABI
        || abi.max_call != kabi::call::MAX as u32
        || abi.table_max != TABLE_MAX as u32
    {
        return Err("abi_info reports another ABI");
    }

    let mut list = [HandleInfo::default(); 4];
    let count = sys::startup(&mut list).map_err(|_| "startup failed")?;
    let sched_rights = Rights::READ | Rights::SET_POLICY | Rights::DUPLICATE | Rights::INSPECT;
    if count != 1 || list[0].kind() != Some(Kind::Sched) || list[0].rights() != sched_rights {
        println!("handles: startup gave {count}: {:?}", &list[..count.min(4)]);
        return Err("startup does not list exactly the Sched handle POLICY gives");
    }
    let sched = Handle(list[0].handle);
    if sys::handle_info(sched) != Ok(list[0]) {
        return Err("handle_info does not read what startup listed");
    }

    expect(
        sys::handle_dup(sched, Rights::READ | Rights::MAP),
        Error::Invalid,
        "a widening dup",
    )?;
    let read = sys::handle_dup(sched, Rights::READ).map_err(|_| "a narrowing dup failed")?;
    let info = sys::handle_info(read).map_err(|_| "handle_info on the dup failed")?;
    if info.kind() != Some(Kind::Sched) || info.rights() != Rights::READ {
        return Err("the dup has other rights than asked");
    }
    expect(
        sys::handle_dup(read, Rights::READ),
        Error::Permission,
        "a dup without DUPLICATE",
    )?;
    expect(
        sys::handle_info(Handle(0x0001_2345)),
        Error::BadHandle,
        "a forged handle",
    )?;
    expect(sys::handle_info(Handle(0)), Error::BadHandle, "handle 0")?;

    let mut stats = Box::new(SchedStats::empty());
    sys::sched_stats(&mut stats, 0)
        .map_err(|_| "sched_stats failed while the Sched handle was open")?;
    sys::handle_close(sched).map_err(|_| "closing the Sched handle failed")?;
    expect(
        sys::sched_stats(&mut stats, 0),
        Error::Permission,
        "sched_stats after the close",
    )?;
    expect(
        sys::handle_info(sched),
        Error::BadHandle,
        "the closed handle",
    )?;
    expect(sys::handle_close(sched), Error::BadHandle, "a second close")?;
    if sys::startup(&mut list) != Ok(0) {
        return Err("startup still lists the closed handle");
    }

    // A replace narrows and closes the old handle, even when refused.
    let none =
        sys::handle_replace(read, Rights::empty()).map_err(|_| "a narrowing replace failed")?;
    expect(
        sys::handle_info(read),
        Error::BadHandle,
        "the replaced handle",
    )?;
    expect(
        sys::handle_replace(none, Rights::READ),
        Error::Invalid,
        "a widening replace",
    )?;
    expect(
        sys::handle_info(none),
        Error::BadHandle,
        "a handle a refused replace closed",
    )?;
    Ok(())
}
