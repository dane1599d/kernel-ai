//! Test program for handles (`test=handles`, ADR 0001 steps 1-3). Started with
//! only the POLICY capability, it checks from ring 3 that:
//!
//! - ABI_INFO names ABI 1.0, the highest call and the table's size;
//! - STARTUP lists one `Sched` handle (READ, SET_POLICY, DUPLICATE, INSPECT,
//!   TRANSFER), the
//!   one POLICY gives, and HANDLE_INFO reads the same;
//! - a duplicate gets a subset of the rights or fails `Invalid`, and one without
//!   DUPLICATE cannot be duplicated again; a forged handle is `BadHandle`;
//! - SCHED_STATS, which needs POLICY, works while the `Sched` handle with
//!   SET_POLICY is open, and fails `Permission` once it is closed, even though a
//!   narrower READ-only duplicate is still open: closing drops the right for good;
//! - a closed handle is `BadHandle`, and STARTUP no longer lists it.
//!
//! With `mode=server` and `mode=client` (task 12) a pair checks handle IPC: the
//! client, started with a route to the server, sends `ping` with a READ-only
//! `Sched` handle through CALL_H; the server's RECV_H reports the client's pid,
//! the route's CALL right and the handle, and REPLY_H sends `pong` and the handle
//! back. The client also checks that a handle without TRANSFER, one of a kind the
//! server may not hold, and more than four handles are refused, and that a
//! forged route or a handle of another kind used as a route is BadHandle.
//!
//! With `mode=grants` (task 13), started with SUPERVISE and POLICY, it makes a
//! grant node under `system`, gives a handle under it, revokes it, and checks
//! that the handle and the node's own handle answer Revoked from then on. With
//! `mode=hold` and `mode=revoke` a pair checks that revoking a grant ends a call
//! in flight through a route given under it: the server holds the call, the
//! kernel revokes the client's node, and the client's CALL_H fails with Revoked
//! long before the server would answer.
//!
//! It prints `handles: every check passed` and exits 0, or names the first check
//! that failed and exits 1.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use kabi::Error;
use kabi::handle::{ABI, Handle, HandleInfo, Kind, RecvInfo, Rights, TABLE_MAX};
use kabi::sched::SchedStats;
use user::{arg, args, println, sys};

user::entry!(main);

fn main() -> i32 {
    let args = args();
    let run = match arg(&args, "mode") {
        Some("server") => server,
        Some("client") => client,
        Some("grants") => grants,
        Some("hold") => hold,
        Some("revoke") => revoke,
        _ => checks,
    };
    match run() {
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

/// The first startup handle of `kind`.
fn startup_handle(kind: Kind) -> Option<Handle> {
    let mut list = [HandleInfo::default(); 8];
    let count = sys::startup(&mut list).ok()?.min(list.len());
    list[..count]
        .iter()
        .find(|info| info.kind() == Some(kind))
        .map(|info| Handle(info.handle))
}

/// Answers one `ping` that carries one handle, with `pong` and that handle.
fn server() -> Result<(), &'static str> {
    let mut buf = [0u8; 64];
    let mut info = RecvInfo::default();
    let len = sys::recv_h(&mut buf, 30_000, &mut info).map_err(|_| "recv_h failed")?;
    if &buf[..len] != b"ping" || info.caller_pid == 0 {
        return Err("recv_h got another request or no caller");
    }
    if !Rights::from_bits_truncate(info.rights).contains(Rights::CALL) || info.handle_count != 1 {
        return Err("recv_h reports other rights or another number of handles");
    }
    let got = Handle(info.handles[0]);
    let held = sys::handle_info(got).map_err(|_| "the moved handle is not in the server")?;
    if held.kind() != Some(Kind::Sched) || held.rights() != Rights::READ | Rights::TRANSFER {
        return Err("the moved handle arrived with another kind or rights");
    }
    sys::reply_h(b"pong", &[got]).map_err(|_| "reply_h failed")?;
    expect(
        sys::handle_info(got),
        Error::BadHandle,
        "the handle sent back",
    )?;
    Ok(())
}

/// The startup `Grant` handle on the `system` root, which SUPERVISE gives.
fn system_grant() -> Result<Handle, &'static str> {
    startup_handle(Kind::Grant).ok_or("no Grant handle on the system root")
}

/// Grant nodes from ring 3; see the module's comment.
fn grants() -> Result<(), &'static str> {
    let root = system_grant()?;
    let sched = startup_handle(Kind::Sched).ok_or("no Sched handle")?;
    let node = sys::grant_new(root, 0, "test").map_err(|_| "grant_new failed")?;
    let given = sys::grant_dup(sched, Rights::READ | Rights::DUPLICATE, node)
        .map_err(|_| "grant_dup failed")?;
    let deeper =
        sys::handle_dup(given, Rights::READ).map_err(|_| "a dup of the given handle failed")?;
    expect(
        sys::grant_new(root, 9, "x"),
        Error::Invalid,
        "an unknown life",
    )?;
    expect(
        sys::grant_revoke(root),
        Error::Permission,
        "revoking the system root",
    )?;
    expect(
        sys::grant_dup(sched, Rights::MAP, node),
        Error::Invalid,
        "a widening grant_dup",
    )?;
    if sys::grant_revoke(node) != Ok(1) {
        return Err("grant_revoke did not revoke exactly its node");
    }
    expect(
        sys::handle_dup(given, Rights::READ),
        Error::Revoked,
        "the handle given under it",
    )?;
    expect(
        sys::handle_dup(deeper, Rights::READ),
        Error::Revoked,
        "a dup of that handle",
    )?;
    expect(
        sys::grant_revoke(node),
        Error::Revoked,
        "the revoked node's own handle",
    )?;
    expect(
        sys::grant_dup(sched, Rights::READ, node),
        Error::Revoked,
        "a grant_dup under it",
    )?;
    if sys::handle_dup(sched, Rights::READ).is_err() {
        return Err("the startup handle went with the node");
    }
    expect(
        sys::once_check(0x7777, &[0; 32]),
        Error::NotFound,
        "a ONCE badge never held",
    )?;
    Ok(())
}

/// Takes one call and holds it for 3 s before answering.
fn hold() -> Result<(), &'static str> {
    let mut buf = [0u8; 16];
    let mut info = RecvInfo::default();
    sys::recv_h(&mut buf, 30_000, &mut info).map_err(|_| "recv_h failed")?;
    sys::sleep_ms(3000);
    // The caller is gone by now; the answer goes nowhere.
    let _ = sys::reply_h(b"late", &[]);
    Ok(())
}

/// Calls the holding server through a route given under its own grant node, which
/// the kernel revokes mid-call.
fn revoke() -> Result<(), &'static str> {
    let node = sys::grant_new(system_grant()?, 0, "call").map_err(|_| "grant_new failed")?;
    let route = user::route(0).ok_or("no route to the server")?;
    let given = sys::grant_dup(route, Rights::CALL, node).map_err(|_| "grant_dup failed")?;
    let mut reply = [0u8; 16];
    let started = sys::time_ns();
    let result = sys::call_h(given, b"wait", &mut reply, 20_000, &[], &mut []);
    let took_ms = (sys::time_ns() - started) / 1_000_000;
    expect(result, Error::Revoked, "a call through a revoked route")?;
    if took_ms >= 2500 {
        return Err("the revoked call waited for the server");
    }
    println!("handles: a revoke ended a call in flight after {took_ms} ms");
    Ok(())
}

/// Calls the server through its route; see the module's comment.
fn client() -> Result<(), &'static str> {
    let route = user::route(0).ok_or("no route to the server")?;
    let sched = startup_handle(Kind::Sched).ok_or("no Sched handle")?;
    let net = startup_handle(Kind::NetDest).ok_or("no NetDest handle")?;
    let mut reply = [0u8; 64];
    let mut back = [Handle::NONE; 4];
    let call = |send: &[Handle], back: &mut [Handle], reply: &mut [u8]| {
        sys::call_h(route, b"ping", reply, 30_000, send, back)
    };
    // Refused before anything moves: no TRANSFER, a kind the server may not
    // hold, more than four, and the same handle twice.
    let plain = sys::handle_dup(sched, Rights::READ).map_err(|_| "dup failed")?;
    expect(
        call(&[plain], &mut back, &mut reply),
        Error::Permission,
        "a handle without TRANSFER",
    )?;
    expect(
        call(&[net], &mut back, &mut reply),
        Error::Permission,
        "a kind the server may not hold",
    )?;
    expect(
        call(&[plain; 5], &mut back, &mut reply),
        Error::TooBig,
        "five handles",
    )?;
    let sent = sys::handle_dup(sched, Rights::READ | Rights::TRANSFER).map_err(|_| "dup failed")?;
    expect(
        call(&[sent, sent], &mut back, &mut reply),
        Error::Invalid,
        "a handle sent twice",
    )?;
    if sys::handle_info(net).is_err() || sys::handle_info(sent).is_err() {
        return Err("a refused call moved a handle");
    }
    expect(
        sys::call_h(
            Handle(0x0007_7777),
            b"ping",
            &mut reply,
            1000,
            &[],
            &mut back,
        ),
        Error::BadHandle,
        "a forged route",
    )?;
    expect(
        sys::call_h(sched, b"ping", &mut reply, 1000, &[], &mut back),
        Error::BadHandle,
        "a Sched handle used as a route",
    )?;
    let (len, came) = call(&[sent], &mut back, &mut reply).map_err(|_| "the call failed")?;
    if &reply[..len] != b"pong" || came != 1 {
        return Err("the reply is not pong with one handle");
    }
    expect(
        sys::handle_info(sent),
        Error::BadHandle,
        "the handle the client sent",
    )?;
    let returned = sys::handle_info(back[0]).map_err(|_| "the returned handle is not open")?;
    if returned.kind() != Some(Kind::Sched) || returned.rights() != Rights::READ | Rights::TRANSFER
    {
        return Err("the returned handle has another kind or rights");
    }
    println!("handles: a request and a handle went to the server and back through a route");
    Ok(())
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
    let sched_rights =
        Rights::READ | Rights::SET_POLICY | Rights::DUPLICATE | Rights::INSPECT | Rights::TRANSFER;
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
