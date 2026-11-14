//! `mode=stop` (`test=shell-interrupt`): started under the name `shell`, it
//! may not interrupt itself without `cap::INPUT`, then calls the holding
//! server, and the kernel's interrupt (the person's Stop) ends that call as
//! Interrupted long before the server would answer. A Stop pressed after
//! that, with no line running, is cleared when the next line is taken.

use kabi::Error;
use user::{println, sys};

/// Longer than the test waits: only the interrupt ends the call in time.
const CALL_MS: u64 = 20_000;
/// The holder answers after 3 s; an interrupted call ends well before.
const LATE_MS: u64 = 2_500;
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
    let result = sys::call_h(route, b"wait", &mut reply, CALL_MS, &[], &mut []);
    let took_ms = (sys::time_ns() - started) / 1_000_000;
    super::expect(result, Error::Interrupted, "a call the person stopped")?;
    if took_ms >= LATE_MS {
        return Err("the stopped call waited for the server");
    }
    // The call's end took the flag: the next line starts clean.
    if sys::interrupted() {
        return Err("the flag outlived the call it ended");
    }
    println!("handles: Stop ended a call in flight after {took_ms} ms, as Interrupted");
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
