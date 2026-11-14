//! `test=async-call`: CALL_SUBMIT and CALL_POLL, as the shell uses them to
//! serve app reads while `llm` answers.
//!
//! - `mode=slow` serves two calls, each answered `pong` after [`SLOW_MS`].
//! - `mode=async`, with a route to it: nothing to poll is NotFound; a call is
//!   submitted, a second is Busy, and a poll is Busy while it is in flight. It
//!   then serves the kernel's `read` in its own `svc_recv` until that returns
//!   Woken, and the poll gives `pong`. A second call with a deadline shorter
//!   than the server's answer wakes it the same way and polls TimedOut.

use kabi::Error;
use kabi::handle::RecvInfo;
use user::{println, sys};

/// How long the slow server takes to answer each call.
const SLOW_MS: u64 = 2_000;
/// The first call's deadline: long enough for the answer.
const CALL_MS: u64 = 10_000;
/// The second call's: shorter than the answer.
const SHORT_MS: u64 = 200;
/// The longest `svc_recv` waits; the call ends long before.
const RECV_MS: u64 = 8_000;

pub fn slow() -> Result<(), &'static str> {
    let mut buf = [0u8; 16];
    for _ in 0..2 {
        let mut info = RecvInfo::default();
        sys::recv_h(&mut buf, 30_000, &mut info).map_err(|_| "recv_h failed")?;
        sys::sleep_ms(SLOW_MS);
        // The second call's caller has given up by now; the answer goes nowhere.
        let _ = sys::reply_h(b"pong", &[]);
    }
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
        sys::call_poll(&mut reply),
        Error::NotFound,
        "a poll with no call",
    )?;
    sys::call_submit(route, b"ping", CALL_MS).map_err(|_| "CALL_SUBMIT failed")?;
    super::expect(
        sys::call_submit(route, b"ping", CALL_MS),
        Error::Busy,
        "a second call while one is in flight",
    )?;
    super::expect(sys::call_poll(&mut reply), Error::Busy, "a poll in flight")?;
    let served = serve_until_woken()?;
    let len = sys::call_poll(&mut reply).map_err(|_| "the poll after Woken failed")?;
    if &reply[..len] != b"pong" {
        return Err("the reply was not the server's");
    }
    if served == 0 {
        return Err("no request was served while the call was in flight");
    }
    super::expect(
        sys::call_poll(&mut reply),
        Error::NotFound,
        "a poll after the call ended",
    )?;
    println!(
        "handles: async: served {served} request while the call was in flight, then polled its reply"
    );
    sys::call_submit(route, b"ping", SHORT_MS).map_err(|_| "the second CALL_SUBMIT failed")?;
    let started = sys::time_ns();
    serve_until_woken()?;
    let took_ms = (sys::time_ns() - started) / 1_000_000;
    super::expect(
        sys::call_poll(&mut reply),
        Error::TimedOut,
        "a call past its deadline",
    )?;
    if took_ms >= SLOW_MS {
        return Err("the deadline did not wake the wait");
    }
    println!(
        "handles: async: a call past its deadline woke the wait after {took_ms} ms and polled TimedOut"
    );
    Ok(())
}
