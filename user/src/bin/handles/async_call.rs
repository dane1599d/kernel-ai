//! `test=async-call`: CALL_SUBMIT and CALL_POLL, as the shell uses them to
//! serve app reads while `llm` answers. The slow servers echo each request, so
//! a late answer that reached the wrong call would show.
//!
//! - `mode=slow` echoes each call after [`SLOW_MS`], until none comes for
//!   [`IDLE_MS`]; `mode=quick` answers one call `quick` at once; `mode=lazy`
//!   takes no call for [`LAZY_MS`], then fails if one is waiting: every call
//!   sent to it is withdrawn first.
//! - `mode=async` (routes: slow, quick, stop, lazy):
//!   1. Nothing to poll is NotFound; a call is submitted, a second is Busy, a
//!      poll is Busy, a blocking call meanwhile is answered as usual, a kernel
//!      call is served from `svc_recv` until Woken, and the poll gives `one`.
//!   2. A call past its short deadline, polled with a wait, is TimedOut.
//!   3. The next call to the same server gets its own answer, not the late one.
//!   4. A Stop while `astop` holds a call: it is still answered, the flag stays,
//!      and nothing is sent after it.
//!   5. A Stop while the call waits for `alazy`: it is withdrawn, Interrupted.
//!   6. A revoke of the route's grant ends a held call at once, Revoked.
//! - `mode=submit-exit` and `mode=submit-hold` send a call to `alazy`, then
//!   exit, or wait to be killed.

use kabi::Error;
use kabi::handle::{RecvInfo, Rights};
use user::{println, sys};

/// How long the slow server takes to answer each call.
const SLOW_MS: u64 = 2_000;
/// How long a slow server waits for another call before it ends.
const IDLE_MS: u64 = 15_000;
/// How long the lazy server takes no call; then how long it checks for one.
const LAZY_MS: u64 = 30_000;
const LAZY_CHECK_MS: u64 = 500;
/// A call's deadline: long enough for any answer.
const CALL_MS: u64 = 20_000;
/// The second call's: shorter than the answer.
const SHORT_MS: u64 = 200;
/// The longest `svc_recv` waits; the call ends long before.
const RECV_MS: u64 = 8_000;
/// A waited poll that only the call's end, not the wait, may end in time.
const POLL_WAIT_MS: u64 = 5_000;
/// What ends well before a server could answer.
const AT_ONCE_MS: u64 = 1_000;

pub fn slow() -> Result<(), &'static str> {
    let mut buf = [0u8; 16];
    loop {
        let mut info = RecvInfo::default();
        // No call for a while: the client is done.
        let Ok(len) = sys::recv_h(&mut buf, IDLE_MS, &mut info) else {
            return Ok(());
        };
        sys::sleep_ms(SLOW_MS);
        // A caller past its deadline has given up by now; the answer goes nowhere.
        let _ = sys::reply_h(&buf[..len.min(buf.len())], &[]);
    }
}

/// `mode=quick`: answers one call `quick` at once.
pub fn quick() -> Result<(), &'static str> {
    let mut buf = [0u8; 16];
    let mut info = RecvInfo::default();
    sys::recv_h(&mut buf, 30_000, &mut info).map_err(|_| "recv_h failed")?;
    sys::reply_h(b"quick", &[]).map_err(|_| "reply_h failed")?;
    Ok(())
}

/// `mode=lazy`: takes no call for [`LAZY_MS`]; every call sent to it must have
/// been withdrawn by then.
pub fn lazy() -> Result<(), &'static str> {
    sys::sleep_ms(LAZY_MS);
    let mut buf = [0u8; 16];
    let mut info = RecvInfo::default();
    match sys::recv_h(&mut buf, LAZY_CHECK_MS, &mut info) {
        Ok(_) => Err("a withdrawn call reached the lazy server"),
        Err(_) => Ok(()),
    }
}

/// `mode=submit-exit`: sends a call to the lazy server and exits.
pub fn submit_exit() -> Result<(), &'static str> {
    let lazy = user::route(0).ok_or("no route to the lazy server")?;
    sys::call_submit(lazy, b"gone", CALL_MS).map_err(|_| "CALL_SUBMIT failed")?;
    Ok(())
}

/// `mode=submit-hold`: sends a call to the lazy server and waits to be killed.
pub fn submit_hold() -> Result<(), &'static str> {
    let lazy = user::route(0).ok_or("no route to the lazy server")?;
    sys::call_submit(lazy, b"killed", CALL_MS).map_err(|_| "CALL_SUBMIT failed")?;
    sys::sleep_ms(CALL_MS);
    Err("the kernel did not kill the holder")
}

/// Serves requests until the call in flight ends (Woken): how many it served.
fn serve_until_woken() -> Result<u32, &'static str> {
    let mut served = 0;
    let mut buf = [0u8; 16];
    loop {
        let mut caller = 0;
        match sys::svc_recv(&mut buf, RECV_MS, &mut caller) {
            Ok(_) => {
                sys::svc_reply(b"served").map_err(|_| "svc_reply failed")?;
                served += 1;
            }
            Err(Error::Woken) => return Ok(served),
            Err(_) => return Err("svc_recv ended without Woken while a call was in flight"),
        }
    }
}

/// Polls with a wait and checks the answer is `want`.
fn answered(want: &[u8], what: &'static str) -> Result<(), &'static str> {
    let mut reply = [0u8; 16];
    let len = sys::call_poll(&mut reply, CALL_MS).map_err(|_| what)?;
    match &reply[..len.min(reply.len())] == want {
        true => Ok(()),
        false => Err(what),
    }
}

/// Polls with a wait and checks it fails `want` well before any answer.
fn refused(want: Error, what: &'static str) -> Result<(), &'static str> {
    let mut reply = [0u8; 16];
    let started = sys::time_ns();
    super::expect(sys::call_poll(&mut reply, POLL_WAIT_MS), want, what)?;
    match (sys::time_ns() - started) / 1_000_000 < AT_ONCE_MS {
        true => Ok(()),
        false => Err(what),
    }
}

pub fn run() -> Result<(), &'static str> {
    first()?;
    let slow = user::route(0).ok_or("no route to the slow server")?;
    // 2 and 3: the late answer to `two` goes nowhere; `three` gets its own.
    sys::call_submit(slow, b"two", SHORT_MS).map_err(|_| "the second CALL_SUBMIT failed")?;
    refused(Error::TimedOut, "a call past its deadline")?;
    sys::call_submit(slow, b"three", CALL_MS).map_err(|_| "the third CALL_SUBMIT failed")?;
    answered(b"three", "the next call did not get its own answer")?;
    println!(
        "handles: async: a call past its deadline polled TimedOut, and its late answer went nowhere"
    );
    stopped()?;
    revoked()
}

/// Step 1: Busy, a blocking call meanwhile, serving until Woken, the reply.
fn first() -> Result<(), &'static str> {
    let slow = user::route(0).ok_or("no route to the slow server")?;
    let mut reply = [0u8; 16];
    super::expect(
        sys::call_poll(&mut reply, 0),
        Error::NotFound,
        "a poll with no call",
    )?;
    sys::call_submit(slow, b"one", CALL_MS).map_err(|_| "CALL_SUBMIT failed")?;
    super::expect(
        sys::call_submit(slow, b"one", CALL_MS),
        Error::Busy,
        "a second call while one is in flight",
    )?;
    super::expect(
        sys::call_poll(&mut reply, 0),
        Error::Busy,
        "a poll in flight",
    )?;
    // A blocking call made meanwhile is neither Busy nor disturbed.
    let quick = user::route(1).ok_or("no route to the quick server")?;
    let (len, _) = sys::call_h(quick, b"q", &mut reply, CALL_MS, &[], &mut [])
        .map_err(|_| "a CALL_H while a call was in flight failed")?;
    if &reply[..len] != b"quick" {
        return Err("the quick server's answer was not its own");
    }
    let served = serve_until_woken()?;
    let len = sys::call_poll(&mut reply, 0).map_err(|_| "the poll after Woken failed")?;
    if &reply[..len] != b"one" || served == 0 {
        return Err("the first call or the serving meanwhile went wrong");
    }
    super::expect(
        sys::call_poll(&mut reply, 0),
        Error::NotFound,
        "a poll after the call ended",
    )?;
    println!(
        "handles: async: served {served} request while the call was in flight, then polled its reply"
    );
    Ok(())
}

/// Steps 4 and 5: the kernel's Stop, while a server holds the call, then
/// while it waits for one.
fn stopped() -> Result<(), &'static str> {
    let stop = user::route(2).ok_or("no route to the stopping server")?;
    sys::call_submit(stop, b"four", CALL_MS).map_err(|_| "the fourth CALL_SUBMIT failed")?;
    answered(b"four", "a taken call was not answered through the Stop")?;
    if !sys::interrupted() {
        return Err("the kernel's Stop never came");
    }
    super::expect(
        sys::call_submit(stop, b"four", CALL_MS),
        Error::Interrupted,
        "a call sent after the Stop",
    )?;
    sys::interrupt_clear();
    let lazy = user::route(3).ok_or("no route to the lazy server")?;
    sys::call_submit(lazy, b"five", CALL_MS).map_err(|_| "the fifth CALL_SUBMIT failed")?;
    refused(Error::Interrupted, "a queued call the person stopped")?;
    sys::interrupt_clear();
    println!(
        "handles: async: a Stop let a taken call answer and withdrew a queued one, and nothing was sent after it"
    );
    Ok(())
}

/// Step 6: the kernel revokes the grant the route was given under.
fn revoked() -> Result<(), &'static str> {
    let root = super::system_grant()?;
    let node = sys::grant_new(root, 0, "async").map_err(|_| "grant_new failed")?;
    let slow = user::route(0).ok_or("no route to the slow server")?;
    let given = sys::grant_dup(slow, Rights::CALL, node).map_err(|_| "grant_dup failed")?;
    sys::call_submit(given, b"six", CALL_MS).map_err(|_| "the sixth CALL_SUBMIT failed")?;
    refused(Error::Revoked, "a call through a revoked route")?;
    println!("handles: async: a revoke ended a held call at once, Revoked");
    Ok(())
}
