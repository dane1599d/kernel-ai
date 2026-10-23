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
//! - Task 13: grant nodes and `ONCE` from the kernel side, grant nodes from ring
//!   3 (`handles mode=grants`), a revoke that ends a call in flight (`mode=hold`
//!   and `mode=revoke`), and spike S01-1's M2: make a request node, give 8
//!   handles to 2 processes, revoke, 1,000 times (pass: p99 at most 50 us).

use crate::fpu_tests::wait_exit;
use crate::handle::grant::{self as grants, Kernel};
use crate::process::{self, MAX_PROCESSES, SpawnRequest};
use crate::{handle, hpet, ipc, log, threads};
use alloc::vec::Vec;
use captable::once::Verdict;
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
    kernel_grants()?;
    ring3_grants()?;
    revoke_mid_call()?;
    m1();
    m2();
    log!("handles: tables, startup handles and need() on handles all behaved");
    Ok(())
}

/// The startup handles each capability bit gives, in a spare slot's table, with
/// one route for CALL.
fn startup_handles() -> Result<(), &'static str> {
    let slot = MAX_PROCESSES - 1;
    let all = cap::GRANTABLE | cap::SUPERVISE;
    handle::create(slot, all, 99, &["llm"]).map_err(|_| "handles: cannot make a table")?;
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
        // PUBLISH and SUPERVISE give two handles each, CALL one route (to llm
        // here), every other bit one handle.
        if count != all.count_ones() as usize + 2 {
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

/// The `Sched` handle in `slot`'s table (it has POLICY).
fn sched_of(slot: usize) -> Result<kabi::handle::Handle, &'static str> {
    handle::with(slot, |h| h.table.find(Kind::Sched, Rights::READ, &Kernel))
        .ok()
        .flatten()
        .ok_or("handles: no Sched handle")
}

/// Grant nodes and `ONCE` handles from the kernel side, in a spare slot's table.
fn kernel_grants() -> Result<(), &'static str> {
    let slot = MAX_PROCESSES - 1;
    handle::create(slot, cap::POLICY, 99, &[]).map_err(|_| "handles: cannot make a table")?;
    let result = (|| {
        let sched = sched_of(slot)?;
        let request = grants::new_request(1).map_err(|_| "handles: no request node")?;
        let given = handle::with(slot, |h| {
            h.table.dup_under(sched, Rights::READ, request, &Kernel)
        })
        .ok()
        .and_then(Result::ok)
        .ok_or("handles: cannot give a handle under a request")?;
        let check = |handle| {
            handle::with(slot, |h| {
                h.table.check(handle, Kind::Sched, Rights::READ, &Kernel)
            })
            .unwrap_or(Err(captable::Error::BadHandle))
        };
        if check(given).is_err() || grants::revoke(request) != Ok(1) {
            return Err("handles: a handle under a live request does not work");
        }
        if check(given) != Err(captable::Error::Revoked)
            || !handle::holds(slot, Kind::Sched, Rights::READ)
        {
            return Err("handles: the revoke did not end exactly the request's handle");
        }
        // ONCE: works once, and its digest matches once, only as shown.
        let request = grants::new_request(2).map_err(|_| "handles: no request node")?;
        let shown = [9u8; 32];
        let once = grants::mint_once(slot, Kind::Sched, Rights::READ, 0, request, shown)
            .map_err(|_| "handles: cannot mint a ONCE handle")?;
        let badge = handle::with(slot, |h| h.table.get(once).map(|e| e.badge))
            .ok()
            .and_then(Result::ok)
            .ok_or("handles: the ONCE handle is not in the table")?;
        if check(once).is_err() || check(once) != Err(captable::Error::BadHandle) {
            return Err("handles: a ONCE handle worked other than once");
        }
        let mut other = shown;
        other[0] ^= 1;
        let verdicts = [
            grants::check_once(badge, &other),
            grants::check_once(badge, &shown),
            grants::check_once(badge, &shown),
        ];
        if verdicts != [Verdict::Mismatch, Verdict::Match, Verdict::Unknown] {
            return Err("handles: a ONCE digest matched other than once and as shown");
        }
        let badge = grants::mint_once(slot, Kind::Sched, Rights::READ, 0, request, shown)
            .and_then(|h| handle::with(slot, |t| t.table.get(h).map(|e| e.badge).unwrap_or(0)))
            .map_err(|_| "handles: cannot mint a second ONCE handle")?;
        let _ = grants::revoke(request);
        if grants::check_once(badge, &shown) != Verdict::Revoked {
            return Err("handles: a ONCE digest outlived its request");
        }
        Ok(())
    })();
    handle::destroy(slot);
    result?;
    log!("handles: request nodes revoke what was given under them; ONCE works once, as shown");
    Ok(())
}

/// `handles mode=grants`, started with SUPERVISE and POLICY.
fn ring3_grants() -> Result<(), &'static str> {
    let pid = process::spawn(&SpawnRequest {
        name: "hgrant",
        program: "handles",
        args: b"mode=grants",
        limits: Limits {
            caps: cap::SUPERVISE | cap::POLICY,
            ..LIMITS
        },
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|_| "handles: cannot start the grants check")?;
    match wait_exit(pid)? {
        (ExitReason::Exited, 0) => Ok(()),
        _ => Err("handles: the ring-3 grant checks failed"),
    }
}

/// A revoke ends a call in flight: `hrev` calls `hhold` through a route it gave
/// under its own grant node; once `hhold` holds the call, the kernel revokes the
/// node, as the trusted path will when a request ends.
fn revoke_mid_call() -> Result<(), &'static str> {
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
        .map_err(|_| "handles: cannot start the revoke pair")
    };
    let holder = start("hhold", "mode=hold", cap::SERVE, &[])?;
    let client = start(
        "hrev",
        "mode=revoke",
        cap::SUPERVISE | cap::CALL,
        &["hhold"],
    )?;
    let mut held = false;
    for _ in 0..1000 {
        if ipc::serving_deadline(holder).is_some() {
            held = true;
            break;
        }
        threads::sleep_ms(10);
    }
    if !held {
        return Err("handles: the holder never took the call");
    }
    let slot = process::with_table(|t| t.iter().position(|p| p.live() && p.pid == client))
        .ok_or("handles: the client is gone")?;
    let node = handle::with(slot, |h| {
        h.table
            .iter()
            .find_map(|(_, e)| (e.kind == Kind::Grant).then_some(e.grant).flatten())
    })
    .ok()
    .flatten()
    .ok_or("handles: the client made no grant node")?;
    grants::revoke(node).map_err(|_| "handles: cannot revoke the client's node")?;
    for pid in [client, holder] {
        if wait_exit(pid)? != (ExitReason::Exited, 0) {
            return Err("handles: the revoke pair's checks failed");
        }
    }
    log!("handles: revoking a grant ended a call in flight through it");
    Ok(())
}

/// M2: make a request node, give 8 handles under it to 2 processes (4 each),
/// revoke it; microseconds per round over 1,000 rounds.
fn m2() {
    const ROUNDS: usize = 1000;
    let slots = [MAX_PROCESSES - 1, MAX_PROCESSES - 2];
    let ready = slots
        .iter()
        .all(|&slot| handle::create(slot, cap::POLICY, 99, &[]).is_ok());
    let sources: Vec<_> = slots
        .iter()
        .filter_map(|&slot| sched_of(slot).ok())
        .collect();
    let mut rounds = Vec::with_capacity(ROUNDS);
    if ready && sources.len() == 2 {
        for n in 0..ROUNDS {
            let start = hpet::nanos();
            let Ok(request) = grants::new_request(n as u64) else {
                break;
            };
            for (&slot, &source) in slots.iter().zip(&sources) {
                for _ in 0..4 {
                    let _ = handle::with(slot, |h| {
                        h.table.dup_under(source, Rights::READ, request, &Kernel)
                    });
                }
            }
            let _ = grants::revoke(request);
            rounds.push(hpet::nanos() - start);
            // Close what the round gave, so the tables stay small.
            for &slot in &slots {
                let _ = handle::with(slot, |h| {
                    let dead: Vec<_> = h
                        .table
                        .iter()
                        .filter(|(_, e)| e.grant.is_some())
                        .map(|(handle, _)| handle)
                        .collect();
                    for handle in dead {
                        let _ = h.table.close(handle);
                    }
                });
            }
        }
    }
    for slot in slots {
        handle::destroy(slot);
    }
    if rounds.len() != ROUNDS {
        log!("handles: M2 not measured");
        return;
    }
    rounds.sort_unstable();
    let us = |ns: u64| alloc::format!("{}.{} us", ns / 1000, ns % 1000 / 100);
    log!(
        "handles: M2: request node, 8 handles to 2 processes, revoke: median {}, p99 {} ({ROUNDS} rounds; {} grant nodes in use after)",
        us(rounds[ROUNDS / 2]),
        us(rounds[ROUNDS * 99 / 100]),
        grants::nodes()
    );
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
