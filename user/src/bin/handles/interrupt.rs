//! `mode=stop` (`test=shell-interrupt`): started under the name `shell`, it
//! may not interrupt itself without `cap::INPUT`. Then, with the person's
//! Stop (the kernel's interrupt) pressed while the holding server has its call
//! in hand:
//!
//! - the taken call is waited for and answered: the server may already be
//!   changing something, and its reply carries the undo record;
//! - the flag stays set for the rest of the line ([`sys::interrupted`]);
//! - a new call in the stopped line ends Interrupted at once, before any server
//!   takes it.
//!
//! A Stop pressed after that, with no line running, is cleared when the next
//! line is taken.

use kabi::Error;
use user::{println, sys};

/// Longer than the test waits: only the holder's answer ends the call in time.
const CALL_MS: u64 = 20_000;
/// The holder answers after 3 s; a taken call is waited for that long.
const HELD_MS: u64 = 2_500;
/// A call in a stopped line ends well before any server could answer.
const AT_ONCE_MS: u64 = 1_000;
/// How long it waits for the kernel's next line.
const RECV_MS: u64 = 10_000;

pub fn stop() -> Result<(), &'static str> {
    super::expect(
        sys::shell_interrupt(),
        Error::Permission,
        "SHELL_INTERRUPT without cap::INPUT",
    )?;
    if sys::interrupted() {
        return Err("a refused SHELL_INTERRUPT set the flag");
    }
    let route = user::route(0).ok_or("no route to the server")?;
    let mut reply = [0u8; 16];
    let started = sys::time_ns();
    let (len, _) = sys::call_h(route, b"wait", &mut reply, CALL_MS, &[], &mut [])
        .map_err(|_| "a taken call was not waited for through the Stop")?;
    let took_ms = (sys::time_ns() - started) / 1_000_000;
    if &reply[..len] != b"late" || took_ms < HELD_MS {
        return Err("a taken call ended before its server answered");
    }
    if !sys::interrupted() {
        return Err("the Stop did not last for the rest of the line");
    }
    let started = sys::time_ns();
    let result = sys::call_h(route, b"again", &mut reply, CALL_MS, &[], &mut []);
    let at_once_ms = (sys::time_ns() - started) / 1_000_000;
    super::expect(result, Error::Interrupted, "a new call in a stopped line")?;
    if at_once_ms >= AT_ONCE_MS {
        return Err("a call in a stopped line waited");
    }
    println!(
        "handles: Stop waited {took_ms} ms for a taken call's answer, then a new call ended Interrupted in {at_once_ms} ms"
    );
    // The kernel presses Stop again with no line running, then sends a line:
    // taking it clears the stale Stop.
    let mut line = [0u8; 16];
    let mut caller = 0;
    sys::svc_recv(&mut line, RECV_MS, &mut caller).map_err(|_| "no next line came")?;
    let answer: &[u8] = match sys::interrupted() {
        true => b"stale",
        false => b"clean",
    };
    sys::svc_reply(answer).map_err(|_| "svc_reply failed")?;
    Ok(())
}
