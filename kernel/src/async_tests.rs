//! `test=async-call`: CALL_SUBMIT and CALL_POLL from ring 3 (see
//! `user/src/bin/handles/async_call.rs` for the client's steps).
//!
//! - A process that exits, and one that is killed, with a call queued for
//!   `alazy`: the call is withdrawn and its slot is free again.
//! - `handles mode=async` against `aslow`, `aquick`, `astop` and `alazy`: the
//!   kernel serves its own call to the client while the client's call is held,
//!   presses Stop while `astop` holds a call and while a call waits for
//!   `alazy`, and revokes the grant node the client's last route was given
//!   under while `aslow` holds that call.
//! - `alazy` takes no call until all of that is over, and fails if one reached
//!   it; afterwards every call slot the test used is free again.

use crate::fpu_tests::wait_exit;
use crate::handle::{self, grant as grants};
use crate::process::{self, SpawnRequest};
use crate::{ipc, log, threads};
use kabi::handle::Kind;
use kabi::{ExitReason, Limits, cap};

const LIMITS: Limits = Limits {
    max_pages: 256,
    max_burst_ms: 10_000,
    caps: 0,
    stack_pages: 0,
};
/// How long a step's state may take to show.
const WAIT_MS: u64 = 15_000;
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

/// Whether `ready()` holds within [`WAIT_MS`].
fn soon(ready: impl Fn() -> bool) -> bool {
    (0..WAIT_MS / POLL_MS).any(|_| {
        let now = ready();
        if !now {
            threads::sleep_ms(POLL_MS);
        }
        now
    })
}

/// Whether the server `pid` has a call in hand.
fn holding(pid: u64) -> bool {
    ipc::serving_deadline(pid).is_some()
}

fn exited(pid: u64) -> Result<(), &'static str> {
    match wait_exit(pid)? {
        (ExitReason::Exited, 0) => Ok(()),
        _ => Err("async-call: a ring-3 process failed its checks"),
    }
}

/// A call queued for `alazy` by a process that exits, then one that is killed:
/// each is withdrawn.
fn exit_and_kill() -> Result<(), &'static str> {
    let gone = start("aexit", "mode=submit-exit", cap::CALL, &["alazy"])?;
    exited(gone)?;
    if ipc::queued_for("alazy") != 0 {
        return Err("async-call: an exited process's call stayed queued");
    }
    let held = start("akill", "mode=submit-hold", cap::CALL, &["alazy"])?;
    if !soon(|| ipc::queued_for("alazy") != 0) {
        return Err("async-call: the holder's call was never queued");
    }
    process::kill(held, ExitReason::Killed).map_err(|_| "async-call: the holder was gone")?;
    let _ = wait_exit(held)?;
    if ipc::queued_for("alazy") != 0 {
        return Err("async-call: a killed process's call stayed queued");
    }
    log!("async-call: an exit and a kill each withdrew the call left queued");
    Ok(())
}

/// The client's grant node, once it made one.
fn client_node(client: u64) -> Option<captable::grant::GrantRef> {
    let slot = process::with_table(|t| t.iter().position(|p| p.live() && p.pid == client))?;
    handle::with(slot, |h| {
        h.table
            .iter()
            .find_map(|(_, e)| (e.kind == Kind::Grant).then_some(e.grant).flatten())
    })
    .ok()
    .flatten()
}

pub fn check() -> Result<(), &'static str> {
    let before = ipc::calls_in_use();
    let lazy = start("alazy", "mode=lazy", cap::SERVE, &[])?;
    exit_and_kill()?;
    let slow = start("aslow", "mode=slow", cap::SERVE, &[])?;
    let quick = start("aquick", "mode=quick", cap::SERVE, &[])?;
    let stop = start("astop", "mode=slow", cap::SERVE, &[])?;
    let client = start(
        "aclient",
        "mode=async",
        cap::SERVE | cap::CALL | cap::SUPERVISE,
        &["aslow", "aquick", "astop", "alazy"],
    )?;
    if !soon(|| holding(slow)) {
        return Err("async-call: the slow server never took the client's call");
    }
    let reply = ipc::call("aclient", b"read", CALL_MS).map_err(|error| {
        log!("async-call: the client did not serve while its call was held: {error:?}");
        "async-call: the client did not serve while its call was held"
    })?;
    if reply != b"served" {
        return Err("async-call: the client answered something else");
    }
    // The person's Stop while `astop` holds the client's call, then while one
    // waits for `alazy`.
    if !soon(|| holding(stop)) {
        return Err("async-call: the stopping server never took the call");
    }
    process::interrupt("aclient").ok_or("async-call: the client was gone")?;
    if !soon(|| ipc::queued_for("alazy") != 0) {
        return Err("async-call: the client's call never waited for the lazy server");
    }
    process::interrupt("aclient").ok_or("async-call: the client was gone")?;
    // The revoke, once `aslow` holds the call sent under the client's node.
    if !soon(|| client_node(client).is_some()) {
        return Err("async-call: the client made no grant node");
    }
    if !soon(|| holding(slow)) {
        return Err("async-call: the slow server never took the call under the node");
    }
    let node = client_node(client).ok_or("async-call: the client's node was gone")?;
    grants::revoke(node).map_err(|_| "async-call: cannot revoke the client's node")?;
    for pid in [client, slow, quick, stop, lazy] {
        exited(pid)?;
    }
    if ipc::calls_in_use() != before {
        log!(
            "async-call: {} call slots in use after, {before} before",
            ipc::calls_in_use()
        );
        return Err("async-call: a call slot stayed taken");
    }
    log!(
        "async-call: a process served a call while its own was in flight; Stop, revoke, exit and kill each freed its call"
    );
    Ok(())
}
