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
//! With `mode=job` (task 14), started with SUPERVISE, POLICY and SCRIPT, it runs
//! spike S01-1's M3 (see [`job`]): `scriptrun` per request, from spawn to revoke.
//!
//! With `mode=shm` (task 14) it serves the kernel's `Shm` case one line at a
//! time (see [`shm`]): it maps the handles the kernel puts in its table, fills and
//! counts the pages, and makes, maps and frees an object of its own.
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
        Some("devlist") => devlist,
        Some("shm") => shm,
        Some("job") => job,
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

/// `mode=shm`: answers the kernel's lines until `exit`. `map <handle> <pages>`
/// maps read-write and answers the address; `fill <byte>` writes every byte of the
/// mapping; `count <byte>` answers how many bytes hold it; `unmap`; and `own` makes
/// a 4-page object with SHM_CREATE, maps, fills, checks and unmaps it. Answers are
/// `ok`, `ok <n>` or `err <what>`.
fn shm() -> Result<(), &'static str> {
    let mut buf = [0u8; 64];
    let mut mapped: Option<(u64, usize)> = None;
    loop {
        let mut caller = 0;
        let len = sys::svc_recv(&mut buf, 60_000, &mut caller).map_err(|_| "shm: no request")?;
        let text = core::str::from_utf8(&buf[..len.min(buf.len())]).unwrap_or("");
        let mut words = text.split_whitespace();
        let number = |word: Option<&str>| word.and_then(|w| w.parse::<u64>().ok());
        let reply = match (words.next(), number(words.next()), number(words.next())) {
            (Some("exit"), ..) => {
                let _ = sys::svc_reply(b"ok");
                return Ok(());
            }
            (Some("map"), Some(handle), Some(pages)) => {
                match sys::shm_map(Handle(handle as u32), true) {
                    Ok(address) => {
                        mapped = Some((address, pages as usize * 4096));
                        alloc::format!("ok {address}")
                    }
                    Err(error) => alloc::format!("err map {error:?}"),
                }
            }
            (Some("fill"), Some(byte), _) => match mapped {
                Some((address, len)) => {
                    region(address, len).fill(byte as u8);
                    alloc::string::String::from("ok")
                }
                None => alloc::string::String::from("err nothing mapped"),
            },
            (Some("count"), Some(byte), _) => match mapped {
                Some((address, len)) => {
                    let n = region(address, len)
                        .iter()
                        .filter(|&&b| b == byte as u8)
                        .count();
                    alloc::format!("ok {n}")
                }
                None => alloc::string::String::from("err nothing mapped"),
            },
            (Some("unmap"), ..) => {
                match mapped.take().map(|(address, _)| sys::shm_unmap(address)) {
                    Some(Ok(_)) => alloc::string::String::from("ok"),
                    Some(Err(error)) => alloc::format!("err unmap {error:?}"),
                    None => alloc::string::String::from("err nothing mapped"),
                }
            }
            (Some("own"), ..) => match own_shm() {
                Ok(()) => alloc::string::String::from("ok"),
                Err(what) => alloc::format!("err {what}"),
            },
            _ => alloc::string::String::from("err unknown line"),
        };
        sys::svc_reply(reply.as_bytes()).map_err(|_| "shm: cannot answer")?;
    }
}

/// M3's rounds.
const ROUNDS: usize = 100;

/// `mode=job`: spike S01-1's M3, `scriptrun` per request. Each round makes a
/// request node, gives a run-tier Sched handle (READ, SET_POLICY) and a 1-page result Shm under it,
/// starts `scriptrun mode=probe` with them moved in, waits for it to exit, reads
/// its result and revokes the node; microseconds per round. A Job with only WAIT
/// cannot start it, and nothing is left to revoke into once a round ends.
fn job() -> Result<(), &'static str> {
    let root = system_grant()?;
    let sched = startup_handle(Kind::Sched).ok_or("no Sched handle")?;
    let starter = scriptrun_job()?;
    let mut times = alloc::vec::Vec::with_capacity(ROUNDS);
    for round in 0..ROUNDS {
        let start = sys::time_ns();
        let node = sys::grant_new(root, 0, "request").map_err(|_| "grant_new failed")?;
        let given = sys::grant_dup(
            sched,
            Rights::READ | Rights::SET_POLICY | Rights::TRANSFER,
            node,
        )
        .map_err(|_| "cannot give the Sched handle")?;
        let result = sys::shm_create(1).map_err(|_| "shm_create failed")?;
        let shared = sys::grant_dup(
            result,
            Rights::MAP | Rights::READ | Rights::WRITE | Rights::TRANSFER,
            node,
        )
        .map_err(|_| "cannot give the result Shm")?;
        let pid = sys::job_spawn(starter, "mode=probe", &[given, shared])
            .map_err(|_| "job_spawn failed")?;
        let exit = sys::job_wait(starter, 30_000)
            .map_err(|_| "job_wait failed")?
            .ok_or("scriptrun did not end")?;
        if exit.pid != pid || exit.reason != kabi::ExitReason::Exited as u64 || exit.detail != 0 {
            println!(
                "handles: scriptrun ended {} {} {}",
                exit.pid, exit.reason, exit.detail
            );
            return Err("scriptrun failed");
        }
        let address = sys::shm_map(result, false).map_err(|_| "cannot map the result")?;
        let ok = region(address, 4096).starts_with(b"ok 2");
        sys::shm_unmap(address).map_err(|_| "cannot unmap the result")?;
        if !ok {
            return Err("scriptrun wrote no result");
        }
        if sys::grant_revoke(node) != Ok(1) {
            return Err("the request node did not revoke");
        }
        sys::handle_close(result).map_err(|_| "close failed")?;
        times.push(sys::time_ns() - start);
        if round == 0 {
            expect(
                sys::job_spawn(sched, "mode=probe", &[]),
                Error::BadHandle,
                "a Sched handle used as a Job",
            )?;
            let waiter = sys::handle_dup(starter, Rights::WAIT).map_err(|_| "dup failed")?;
            expect(
                sys::job_spawn(waiter, "mode=probe", &[]),
                Error::Permission,
                "a Job with only WAIT",
            )?;
        }
    }
    times.sort_unstable();
    let us = |at: usize| times[(at * (ROUNDS - 1)) / 100] / 1000;
    println!(
        "handles: M3: scriptrun per request (spawn, run, exit, revoke): p50 {} us, p99 {} us, max {} us over {ROUNDS} rounds",
        us(50),
        us(99),
        times[ROUNDS - 1] / 1000
    );
    Ok(())
}

/// The startup Job that starts `scriptrun`: CREATE and WAIT.
fn scriptrun_job() -> Result<Handle, &'static str> {
    let mut list = [HandleInfo::default(); 8];
    let count = sys::startup(&mut list)
        .map_err(|_| "no STARTUP")?
        .min(list.len());
    list[..count]
        .iter()
        .find(|info| info.kind() == Some(Kind::Job) && info.rights().contains(Rights::CREATE))
        .map(|info| Handle(info.handle))
        .ok_or("no scriptrun Job")
}

/// The mapping at `address` as bytes.
fn region(address: u64, len: usize) -> &'static mut [u8] {
    // SAFETY: SHM_MAP mapped `len` bytes read-write at `address` for this process,
    // and only this thread touches them until `unmap`.
    unsafe { core::slice::from_raw_parts_mut(address as *mut u8, len) }
}

/// SHM_CREATE from ring 3: 4 pages, mapped twice is Busy, filled and read back,
/// unmapped; then a map of a closed handle fails.
fn own_shm() -> Result<(), &'static str> {
    let handle = sys::shm_create(4).map_err(|_| "create failed")?;
    let address = sys::shm_map(handle, true).map_err(|_| "map failed")?;
    expect(
        sys::shm_map(handle, false),
        Error::Busy,
        "a second map of one object",
    )?;
    let bytes = region(address, 4 * 4096);
    if bytes.iter().any(|&b| b != 0) {
        return Err("new pages were not zero");
    }
    bytes.fill(0x5A);
    if bytes.iter().any(|&b| b != 0x5A) {
        return Err("the pages did not keep what was written");
    }
    sys::shm_unmap(address).map_err(|_| "unmap failed")?;
    expect(sys::shm_create(0), Error::Invalid, "a 0-page object")?;
    sys::handle_close(handle).map_err(|_| "close failed")?;
    expect(
        sys::shm_map(handle, true),
        Error::BadHandle,
        "a closed handle",
    )?;
    Ok(())
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

/// `handles mode=devlist`: DEV_LIST from ring 3 (ADR 0007), one line a device.
fn devlist() -> Result<(), &'static str> {
    use kabi::device::{DeviceRecord, MAX_RECORDS, irq_mode, place, state};
    let mut records = [DeviceRecord::default(); MAX_RECORDS];
    let count = sys::dev_list(&mut records).map_err(|_| "DEV_LIST failed")?;
    if count == 0 {
        return Err("DEV_LIST listed no device");
    }
    for r in &records[..count] {
        let place = match r.place {
            place::KERNEL => "kernel",
            _ => "user",
        };
        let state = match r.state {
            state::RUNNING => "running",
            state::FAILED => "failed",
            _ => "bound",
        };
        let irq = match r.irq_mode {
            irq_mode::MSI_X => "MSI-X",
            irq_mode::MSI => "MSI",
            _ => "polled",
        };
        println!(
            "devlist: {} {:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}{:02x}{:02x} {} in the {place}, {state}, {irq}, {} interrupts",
            r.index,
            r.bus,
            r.device_number(),
            r.function(),
            r.vendor,
            r.device,
            r.class[0],
            r.class[1],
            r.class[2],
            r.driver(),
            r.interrupts
        );
    }
    let mut one = [DeviceRecord::default(); 1];
    let first = DeviceRecord {
        interrupts: one[0].interrupts,
        ..records[0]
    };
    if sys::dev_list(&mut one) != Ok(1)
        || one[0]
            != (DeviceRecord {
                interrupts: one[0].interrupts,
                ..first
            })
    {
        return Err("DEV_LIST into a one-record buffer did not give the first record");
    }
    Ok(())
}
