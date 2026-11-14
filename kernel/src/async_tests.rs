//! `test=async-call`: CALL_SUBMIT and CALL_POLL from ring 3. `handles
//! mode=async` sends a call to `handles mode=slow`, which holds it 2 s; while
//! it is held, the kernel calls the client, which serves it from the same
//! thread, then is woken by the reply and polls it. A second call past its
//! deadline wakes it too and polls TimedOut (see `user/src/bin/handles`).

use crate::fpu_tests::wait_exit;
use crate::process::{self, SpawnRequest};
use crate::{ipc, log, threads};
use kabi::{ExitReason, Limits, cap};

const LIMITS: Limits = Limits {
    max_pages: 256,
    max_burst_ms: 10_000,
    caps: 0,
    stack_pages: 0,
};
/// How long the slow server may take to have the call in hand.
const HELD_WAIT_MS: u64 = 10_000;
const POLL_MS: u64 = 10;
/// The kernel's call to the client, served while the client's own is held.
const CALL_MS: u64 = 5_000;

fn start(
    name: &'static str,
    args: &'static str,
    caps: u64,
    uses: &'static [&'static str],
) -> Result<u64, &'static str> {
    process::spawn(&SpawnRequest {
        name,
        program: "handles",
        args: args.as_bytes(),
        limits: Limits { caps, ..LIMITS },
        spawner: 0,
        supervisor: false,
        uses,
    })
    .map_err(|_| "async-call: cannot start a handles process")
}

pub fn check() -> Result<(), &'static str> {
    let slow = start("aslow", "mode=slow", cap::SERVE, &[])?;
    let client = start("aclient", "mode=async", cap::SERVE | cap::CALL, &["aslow"])?;
    let held = (0..HELD_WAIT_MS / POLL_MS).any(|_| {
        let serving = ipc::serving_deadline(slow).is_some();
        if !serving {
            threads::sleep_ms(POLL_MS);
        }
        serving
    });
    if !held {
        return Err("async-call: the slow server never took the client's call");
    }
    let reply = ipc::call("aclient", b"read", CALL_MS).map_err(|error| {
        log!("async-call: the client did not serve while its call was held: {error:?}");
        "async-call: the client did not serve while its call was held"
    })?;
    if reply != b"served" {
        return Err("async-call: the client answered something else");
    }
    for pid in [client, slow] {
        if wait_exit(pid)? != (ExitReason::Exited, 0) {
            return Err("async-call: the ring-3 checks failed");
        }
    }
    log!("async-call: a process served a call while its own was in flight, and polled the reply");
    Ok(())
}
