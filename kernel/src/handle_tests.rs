//! `test=handles` (P1 task 5, ADR 0001 steps 1-3): handle tables in the kernel.
//!
//! - Every process gets a table at spawn, with startup handles from its capability
//!   bits, and loses it when it ends.
//! - The ring-3 program `handles`, started with POLICY alone, checks ABI_INFO,
//!   STARTUP, HANDLE_INFO, DUP, REPLACE and CLOSE, and that a call needing POLICY
//!   fails once its `Sched` handle is closed (see `user/src/bin/handles.rs`).
//! - Spike S01-1's M1: the extra cost of a handle check over a bit check, over
//!   100,000 checks each. It is recorded, not judged: the marks (100 ns under KVM)
//!   are read from the log.

use crate::fpu_tests::wait_exit;
use crate::process::{self, MAX_PROCESSES, SpawnRequest};
use crate::{handle, hpet, log};
use core::hint::black_box;
use kabi::handle::{Kind, Rights};
use kabi::{ExitReason, Limits, cap};

const CHECKS: u64 = 100_000;

const LIMITS: Limits = Limits {
    max_pages: 256,
    max_burst_ms: 10_000,
    caps: cap::POLICY,
    stack_pages: 0,
};

pub fn check() -> Result<(), &'static str> {
    startup_handles()?;
    ring3()?;
    routes()?;
    m1();
    log!("handles: tables, startup handles and need() on handles all behaved");
    Ok(())
}

/// The startup handles each capability bit gives, in a spare slot's table.
fn startup_handles() -> Result<(), &'static str> {
    let slot = MAX_PROCESSES - 1;
    let all = cap::GRANTABLE | cap::SUPERVISE;
    handle::create(slot, all, 99, &[]).map_err(|_| "handles: cannot make a table")?;
    let result = (|| {
        for bit in (0..64).map(|n| 1u64 << n).filter(|bit| all & bit != 0) {
            let (kind, rights) =
                handle::required(bit).ok_or("handles: a capability bit has no handle")?;
            if !handle::holds(slot, kind, rights) {
                return Err("handles: a startup handle is missing");
            }
            handle::need(slot, bit).map_err(|_| "handles: need() refused a held right")?;
        }
        let count = handle::with(slot, |handles| handles.startup().count()).unwrap_or(0);
        // PUBLISH gives two handles, every other bit one.
        if count != all.count_ones() as usize + 1 {
            return Err("handles: the wrong number of startup handles");
        }
        Ok(())
    })();
    handle::destroy(slot);
    result?;
    if handle::need(slot, cap::POLICY).is_ok() {
        return Err("handles: a destroyed table still grants a right");
    }
    handle::create(slot, cap::NET, 99, &[]).map_err(|_| "handles: cannot make a table")?;
    let refused = handle::need(slot, cap::RESOLVE).is_err()
        && handle::need(slot, cap::NET | cap::POLICY).is_err()
        && handle::need(slot, cap::NET).is_ok()
        && !handle::holds(slot, Kind::NetDest, Rights::RESOLVE);
    handle::destroy(slot);
    if !refused {
        return Err("handles: NET alone granted more than CONNECT to llm=");
    }
    log!("handles: each capability bit gives its startup handle, and only that");
    Ok(())
}

/// Runs `handles` in ring 3 and checks it exited 0.
fn ring3() -> Result<(), &'static str> {
    let pid = process::spawn(&SpawnRequest {
        name: "handles",
        program: "handles",
        args: &[],
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|_| "handles: cannot start the handles program")?;
    match wait_exit(pid)? {
        (ExitReason::Exited, 0) => Ok(()),
        _ => Err("handles: the ring-3 checks failed"),
    }
}

/// Task 12: `handles mode=client`, with a route to `hsrv`, sends a request and a
/// handle to `handles mode=server` through CALL_H and gets both back.
fn routes() -> Result<(), &'static str> {
    let start = |name, args: &'static str, caps, uses: &'static [&'static str]| {
        process::spawn(&SpawnRequest {
            name,
            program: "handles",
            args: args.as_bytes(),
            limits: Limits { caps, ..LIMITS },
            spawner: 0,
            supervisor: false,
            uses,
        })
        .map_err(|_| "handles: cannot start the route pair")
    };
    let server = start("hsrv", "mode=server", cap::SERVE | cap::POLICY, &[])?;
    let client = start(
        "hcli",
        "mode=client",
        cap::CALL | cap::POLICY | cap::NET,
        &["hsrv"],
    )?;
    for pid in [client, server] {
        if wait_exit(pid)? != (ExitReason::Exited, 0) {
            return Err("handles: the route pair's checks failed");
        }
    }
    log!("handles: CALL_H, RECV_H and REPLY_H moved a request and a handle there and back");
    Ok(())
}

/// M1: nanoseconds per check, the old bit check (a copy of the process under the
/// table lock, as `need` did) against the handle check `need` does now.
fn m1() {
    let slot = MAX_PROCESSES - 1;
    if handle::create(slot, cap::GRANTABLE | cap::SUPERVISE, 99, &[]).is_err() {
        log!("handles: M1 not measured: no table");
        return;
    }
    let time = |f: &dyn Fn() -> bool| {
        let start = hpet::nanos();
        let mut held = 0u64;
        for _ in 0..CHECKS {
            held += u64::from(black_box(f()));
        }
        black_box(held);
        (hpet::nanos() - start) / CHECKS
    };
    let bits = time(&|| process::identity(black_box(slot)).limits.caps & cap::POLICY == 0);
    let handles = time(&|| handle::need(black_box(slot), cap::POLICY).is_ok());
    handle::destroy(slot);
    log!(
        "handles: M1: bit check {bits} ns, handle check {handles} ns per call, {} ns more ({CHECKS} checks each)",
        handles.saturating_sub(bits)
    );
}
