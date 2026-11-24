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
    agent_route()?;
    kernel_grants()?;
    ring3_grants()?;
    revoke_mid_call()?;
    held_move()?;
    m1();
    m2();
    log!("handles: tables, startup handles and need() on handles all behaved");
    Ok(())
}

/// ADR 0013 section 1: a plan line's `agent=` badge rides on its route to
/// `llm`, and only there, and that route is pinned: CALL and INSPECT, with no
/// TRANSFER or DUPLICATE, so no process can pass the badge on. A line with
/// none has badge 0 and the usual rights on every route.
fn agent_route() -> Result<(), &'static str> {
    use kabi::handle::agent;
    type Route = Option<(u64, Rights)>;
    let routes = |grants: supervise::Grants| -> Result<(Route, Route), &'static str> {
        let request = SpawnRequest {
            name: "hagent",
            program: "handles",
            args: b"mode=idle",
            limits: Limits {
                caps: cap::CALL,
                ..LIMITS
            },
            spawner: 0,
            supervisor: false,
            uses: &[agent::SERVICE, "hsrv"],
        };
        let pid = process::spawn_in(&request, Vec::new(), false, &grants)
            .map_err(|_| "handles: cannot start the agent's process")?;
        let slot = process::slot_of_pid(pid).ok_or("handles: the agent's process is gone")?;
        let found = (
            handle::route_of(slot, agent::SERVICE),
            handle::route_of(slot, "hsrv"),
        );
        process::kill(pid, ExitReason::Killed)
            .map_err(|_| "handles: cannot end the agent's process")?;
        wait_exit(pid)?;
        Ok(found)
    };
    let usual = Rights::CALL | Rights::DUPLICATE | Rights::INSPECT | Rights::TRANSFER;
    let kai = supervise::Grants {
        agent: Some(agent::KAI),
        pinned: alloc::vec![agent::SERVICE.into()],
        ..supervise::Grants::default()
    };
    let (llm, other) = routes(kai)?;
    if llm != Some((agent::KAI, handle::PINNED_RIGHTS)) || other != Some((0, usual)) {
        return Err("handles: an agent's badge is not on its pinned route to llm alone");
    }
    if handle::PINNED_RIGHTS.intersects(Rights::TRANSFER | Rights::DUPLICATE) {
        return Err("handles: a pinned route could be passed on");
    }
    if routes(supervise::Grants::default())? != (Some((0, usual)), Some((0, usual))) {
        return Err("handles: a line with no agent= has a badge or a pinned route");
    }
    log!(
        "handles: an agent's route to llm carries its badge (kai {}) with CALL only, and no other route does",
        agent::KAI
    );
    Ok(())
}

/// The startup handles each capability bit gives, in a spare slot's table, with
/// one route for CALL.
fn startup_handles() -> Result<(), &'static str> {
    let slot = MAX_PROCESSES - 1;
    let all = cap::GRANTABLE | cap::SUPERVISE;
    handle::create(slot, all, 99, &["llm"]).map_err(|_| "handles: cannot make a table")?;
    let result = (|| {
        // NET gives no handle of its own: it is the ceiling for the NetDest
        // handles a plan line's `netdest=` gives (K, after #756).
        let given = all & !cap::NET;
        for bit in (0..64).map(|n| 1u64 << n).filter(|bit| given & bit != 0) {
            let (kind, rights) =
                handle::required(bit).ok_or("handles: a capability bit has no handle")?;
            if !handle::holds(slot, kind, rights) {
                return Err("handles: a startup handle is missing");
            }
            handle::need(slot, bit).map_err(|_| "handles: need() refused a held right")?;
        }
        let count = handle::with(slot, |handles| handles.startup().count()).unwrap_or(0);
        // PUBLISH and SUPERVISE give two handles each, CALL one route (to llm
        // here), NET none, every other bit one handle.
        if count != given.count_ones() as usize + 2 {
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
    // Two claims, failed apart: NET alone puts no NetDest in the table, and
    // need() refuses NET (and anything naming it) without one.
    let held = handle::holds(slot, Kind::NetDest, Rights::CONNECT);
    let needed = handle::need(slot, cap::RESOLVE).is_ok()
        || handle::need(slot, cap::NET | cap::POLICY).is_ok()
        || handle::need(slot, cap::NET).is_ok();
    handle::destroy(slot);
    if held {
        return Err("handles: NET alone gave a NetDest handle");
    }
    if needed {
        return Err("handles: need() passed NET with no NetDest held");
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
    let start = |name, args: &'static str, caps, uses: &'static [&'static str], moved| {
        let request = SpawnRequest {
            name,
            program: "handles",
            args: args.as_bytes(),
            limits: Limits { caps, ..LIMITS },
            spawner: 0,
            supervisor: false,
            uses,
        };
        process::spawn_with(&request, moved, false)
            .map_err(|_| "handles: cannot start the route pair")
    };
    let server = start(
        "hsrv",
        "mode=server",
        cap::SERVE | cap::POLICY,
        &[],
        Vec::new(),
    )?;
    // A NetDest the client may move but the server may not hold: NET gives no
    // handle of its own (K, after #756), so the kernel moves one in.
    let net = captable::Entry::new(
        Kind::NetDest,
        Rights::CONNECT | Rights::TRANSFER,
        kabi::handle::netdest::LLM,
    );
    let client = start(
        "hcli",
        "mode=client",
        cap::CALL | cap::POLICY | cap::NET,
        &["hsrv"],
        alloc::vec![net],
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
        // ONCE_CHECK spends only a ONCE the caller holds (audit K #6): from
        // another table it is NotFound and leaves it, then its holder uses it.
        let elsewhere = (slot + 1) % MAX_PROCESSES;
        if grants::spend_held_once(elsewhere, badge, &shown) != Err(kabi::Error::NotFound)
            || grants::check_once(badge, &other) != Verdict::Mismatch
        {
            return Err("handles: a ONCE was checked by a process that does not hold it");
        }
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

/// `handles mode=grants`, started with SUPERVISE and POLICY. It exits owning
/// a live node, so its exit only completes if `revoke_owned` does not
/// deadlock (audit K #11).
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
    let ended = wait_exit(pid)?;
    // PROC_ALIVE's answer for a process that ran and has ended: gone.
    if process::alive(pid) {
        return Err("handles: an ended process still answered alive");
    }
    match ended {
        (ExitReason::Exited, 0) if grants::owns_any(pid) => {
            Err("handles: a node the ended process made is still live")
        }
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
    unanswered()?;
    crate::ipc_queued_tests::check()?;
    log!("handles: revoking a grant ended a call in flight through it");
    Ok(())
}

/// Audit K #19: a call its server took and did not answer by the deadline
/// ends Unanswered, not TimedOut, so APP_CALL can say it may have run and the
/// shell never sends it again. `hold` takes one call and keeps it 3 s.
fn unanswered() -> Result<(), &'static str> {
    const NAME: &str = "hunans";
    let pid = process::spawn(&SpawnRequest {
        name: NAME,
        program: "handles",
        args: b"mode=hold",
        limits: Limits {
            caps: cap::SERVE,
            ..LIMITS
        },
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|_| "handles: cannot start the holder")?;
    // Bound and waiting, so the call is taken at once rather than queued.
    for _ in 0..1000 {
        if ipc::server_of(NAME).is_some() {
            break;
        }
        threads::sleep_ms(10);
    }
    let result = ipc::call_from(NAME, b"hold", 500);
    if wait_exit(pid)? != (ExitReason::Exited, 0) {
        return Err("handles: the holder failed");
    }
    match result {
        Err(ipc::CallError::Unanswered) => Ok(()),
        _ => Err("handles: a call taken and not answered did not end Unanswered"),
    }
}

/// #1162's review: a refused JOB_SPAWN gives the caller its handles back
/// under the values it holds, with their rights, even when another insert
/// lands in the table between the take and the give-back; a move that went
/// through closes them, and so does a refusal that brings nothing back.
fn held_move() -> Result<(), &'static str> {
    let slot = MAX_PROCESSES - 3;
    handle::create(slot, cap::POLICY, 98, &[]).map_err(|_| "handles: no table for the move")?;
    let result = (|| {
        let rights = Rights::READ | Rights::TRANSFER;
        let insert = |rights| {
            handle::with(slot, |h| {
                h.table.insert(captable::Entry::new(Kind::Sched, rights, 1))
            })
            .ok()
            .and_then(Result::ok)
            .ok_or("handles: an insert failed")
        };
        let moved = insert(rights)?;
        let moving = handle::take_held(slot, &[moved], cap::POLICY)
            .map_err(|_| "handles: a held take was refused")?;
        // A sibling thread's insert while the spawn is under way.
        let between = insert(Rights::READ)?;
        if between.index() == moved.index() {
            return Err("handles: an insert took a slot held for a move");
        }
        handle::give_back(slot, &[moved], moving);
        let back = handle::with(slot, |h| h.table.get(moved).map(|e| e.rights)).ok();
        if back != Some(Ok(rights)) {
            return Err("handles: a refused move's handle did not come back as it was");
        }
        let moving = handle::take_held(slot, &[moved], cap::POLICY)
            .map_err(|_| "handles: a second held take was refused")?;
        handle::release(slot, &[moved]);
        drop(moving);
        // `hold` already emptied the slot, so only its reuse shows the release:
        // the next insert takes the freed slot under a new value.
        let reused = insert(Rights::READ)?;
        if reused.index() != moved.index() || reused == moved {
            return Err("handles: a move that went through left its slot held");
        }
        if handle::with(slot, |h| h.table.get(moved).is_err()).ok() != Some(true) {
            return Err("handles: a move that went through left its old value open");
        }
        // A refused spawn whose child's table went with the entries (#1267's
        // review): nothing comes back, and the held slot is closed, not leaked.
        let lost = insert(rights)?;
        let moving = handle::take_held(slot, &[lost], cap::POLICY)
            .map_err(|_| "handles: a third held take was refused")?;
        drop(moving);
        handle::give_back(slot, &[lost], handle::Moving::none());
        let reused = insert(Rights::READ)?;
        if reused.index() != lost.index() || reused == lost {
            return Err("handles: a refused move with nothing back left its slot held");
        }
        Ok(())
    })();
    handle::destroy(slot);
    result?;
    log!(
        "handles: a refused move's handle came back under its value with its rights, an insert meanwhile notwithstanding"
    );
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
