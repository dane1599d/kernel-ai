//! `test=proc-stop` (P1 task 15, lane K): the two ways to stop a process.
//! - PROC_STOP, for a Job handle with STOP (only the shell has one): it stops the
//!   plan's `unsupervised` `ticker` for good, and refuses llm, the shell, the
//!   supervised `spin` and a name nothing has.
//! - The supervisor's `stop` and `start` requests: `spin` stays down past its
//!   restart backoff until `start`, and each refusal has its own reply.

use crate::process::{self, supervision};
use crate::{ipc, log, threads};
use alloc::string::String;
use kabi::Error;

/// The plan lines this case adds to the shell and llm.
pub const PLAN: &str = "spin program=spinner burst_ms=3600000\n\
     ticker program=spinner burst_ms=3600000 unsupervised";
const SERVICE: &str = "spin";
const UNSUPERVISED: &str = "ticker";
const START_WAIT_MS: u64 = 60_000;
const STOP_WAIT_MS: u64 = 5_000;
/// Longer than the supervisor's first restart backoff, so a restart would show.
const STAYS_DOWN_MS: u64 = 1_000;

pub fn check() -> Result<(), &'static str> {
    wait_within(START_WAIT_MS, || {
        ipc::server_of("shell").is_some() && running(SERVICE) && running(UNSUPERVISED)
    })
    .map_err(|_| "proc-stop: the shell, spin and ticker did not all come up")?;
    // PROC_STOP as the shell meets it.
    for (name, want) in [
        ("llm", Err(Error::Permission)),
        ("shell", Err(Error::Permission)),
        ("supervisor", Err(Error::Permission)),
        (SERVICE, Err(Error::Invalid)),
        ("nobody", Err(Error::NotFound)),
        (UNSUPERVISED, Ok(1)),
    ] {
        let got = supervision::stop_named("shell", name);
        if got != want {
            log!("proc-stop: proc_stop({name}) gave {got:?}, wanted {want:?}");
            return Err("proc-stop: PROC_STOP gave the wrong answer");
        }
    }
    wait_within(STOP_WAIT_MS, || !running(UNSUPERVISED))
        .map_err(|_| "proc-stop: ticker did not end")?;
    // The supervisor's requests, refusals first.
    for (request, want) in [
        ("stop llm", "err protected"),
        ("start shell", "err protected"),
        ("stop nobody", "err unknown service"),
        ("start spin", "err not stopped"),
        ("restart spin", "err unknown request"),
        ("stop spin", "ok"),
    ] {
        ask(request, want)?;
    }
    wait_within(STOP_WAIT_MS, || !running(SERVICE))
        .map_err(|_| "proc-stop: spin did not end on stop")?;
    threads::sleep_ms(STAYS_DOWN_MS);
    if running(SERVICE) || running(UNSUPERVISED) {
        return Err("proc-stop: a stopped process came back");
    }
    ask("stop spin", "err already stopped")?;
    ask("start spin", "ok")?;
    wait_within(START_WAIT_MS, || running(SERVICE))
        .map_err(|_| "proc-stop: spin did not start again")?;
    log!("proc-stop: PROC_STOP and the supervisor's stop and start kept to their rules");
    Ok(())
}

fn running(name: &str) -> bool {
    !process::named(name).is_empty()
}

fn ask(request: &str, want: &str) -> Result<(), &'static str> {
    let reply = ipc::call("supervisor", request.as_bytes(), 5_000)
        .map_err(|_| "proc-stop: the supervisor did not answer")?;
    if reply != want.as_bytes() {
        log!(
            "proc-stop: {request:?} -> {:?}, wanted {want:?}",
            String::from_utf8_lossy(&reply)
        );
        return Err("proc-stop: the supervisor gave the wrong reply");
    }
    Ok(())
}

fn wait_within(ms: u64, mut done: impl FnMut() -> bool) -> Result<(), ()> {
    for _ in 0..ms / 10 {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    Err(())
}
