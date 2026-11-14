//! `test=async-call`: CALL_SUBMIT and CALL_POLL, as the shell uses them to
//! serve app reads while `llm` answers.
//!
//! - `mode=slow` answers each call `pong` after [`SLOW_MS`], until none comes
//!   for [`IDLE_MS`].
//! - `mode=async`, with a route to it: nothing to poll is NotFound; a call is
//!   submitted, a second is Busy, and a poll is Busy while it is in flight; a
//!   blocking call to `mode=quick` meanwhile is answered as usual. It
//!   then serves the kernel's `read` in its own `svc_recv` until that returns
//!   Woken, and the poll gives `pong`. A second call with a deadline shorter
//!   than the server's answer, polled with a 5 s wait, answers TimedOut once
//!   the deadline passes. Last, the kernel presses Stop while `astop` holds a
//!   third call: it is still answered, and no call is sent after the Stop.

use kabi::Error;
use kabi::handle::RecvInfo;
use user::{println, sys};

/// How long the slow server takes to answer each call.
const SLOW_MS: u64 = 2_000;
/// How long a slow server waits for another call before it ends.
const IDLE_MS: u64 = 15_000;
/// The first call's deadline: long enough for the answer.
const CALL_MS: u64 = 10_000;
/// The second call's: shorter than the answer.
const SHORT_MS: u64 = 200;
/// The longest `svc_recv` waits; the call ends long before.
const RECV_MS: u64 = 8_000;
/// The second call's waited poll: longer than the server, so only the
/// deadline ends it in time.
const POLL_WAIT_MS: u64 = 5_000;

pub fn slow() -> Result<(), &'static str> {
    let mut buf = [0u8; 16];
    loop {
        let mut info = RecvInfo::default();
        // No call for a while: the client is done.
        if sys::recv_h(&mut buf, IDLE_MS, &mut info).is_err() {
            return Ok(());
        }
        sys::sleep_ms(SLOW_MS);
        // A caller past its deadline has given up by now; the answer goes nowhere.
        let _ = sys::reply_h(b"pong", &[]);
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

pub fn run() -> Result<(), &'static str> {
    let route = user::route(0).ok_or("no route to the server")?;
    let mut reply = [0u8; 16];
    super::expect(
        sys::call_poll(&mut reply, 0),
        Error::NotFound,
        "a poll with no call",
    )?;
    sys::call_submit(route, b"ping", CALL_MS).map_err(|_| "CALL_SUBMIT failed")?;
    super::expect(
        sys::call_submit(route, b"ping", CALL_MS),
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
    super::expect(
        sys::call_poll(&mut reply, 0),
        Error::Busy,
        "a poll after the CALL_H",
    )?;
    let served = serve_until_woken()?;
    let len = sys::call_poll(&mut reply, 0).map_err(|_| "the poll after Woken failed")?;
    if &reply[..len] != b"pong" {
        return Err("the reply was not the server's");
    }
    if served == 0 {
        return Err("no request was served while the call was in flight");
    }
    super::expect(
        sys::call_poll(&mut reply, 0),
        Error::NotFound,
        "a poll after the call ended",
    )?;
    println!(
        "handles: async: served {served} request while the call was in flight, then polled its reply"
    );
    sys::call_submit(route, b"ping", SHORT_MS).map_err(|_| "the second CALL_SUBMIT failed")?;
    let started = sys::time_ns();
    super::expect(
        sys::call_poll(&mut reply, POLL_WAIT_MS),
        Error::TimedOut,
        "a waited poll of a call past its deadline",
    )?;
    let took_ms = (sys::time_ns() - started) / 1_000_000;
    if took_ms >= SLOW_MS {
        return Err("the deadline did not end the waited poll");
    }
    println!(
        "handles: async: a waited poll of a call past its deadline answered TimedOut after {took_ms} ms"
    );
    stopped()
}

/// The person's Stop (the kernel's interrupt) while `astop` holds the call: the
/// taken call is still answered, the flag stays, and a new call is not sent.
fn stopped() -> Result<(), &'static str> {
    let route = user::route(2).ok_or("no route to the stopping server")?;
    let mut reply = [0u8; 16];
    sys::call_submit(route, b"ping", CALL_MS).map_err(|_| "the third CALL_SUBMIT failed")?;
    let len = sys::call_poll(&mut reply, CALL_MS)
        .map_err(|_| "a taken call was not answered through the Stop")?;
    if &reply[..len] != b"pong" {
        return Err("the taken call's answer was not its server's");
    }
    if !sys::interrupted() {
        return Err("the kernel's Stop never came");
    }
    super::expect(
        sys::call_submit(route, b"ping", CALL_MS),
        Error::Interrupted,
        "a call sent after the Stop",
    )?;
    println!("handles: async: a Stop let the taken call answer, and no call was sent after it");
    Ok(())
}
