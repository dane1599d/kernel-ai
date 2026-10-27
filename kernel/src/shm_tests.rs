//! `test=shm` (P1 task 14, ADR 0001 D4): shared pages, and spike S01-1's M6.
//!
//! - Two `handles mode=shm` processes each make, map, fill and free a 4-page
//!   object of their own (SHM_CREATE from ring 3).
//! - The kernel makes a 256-page object. Each round, it puts a handle to it under
//!   a new request node into both processes' tables, both map it, and the kernel
//!   revokes the node: M6's time, TLB shootdown on every CPU included. On the first
//!   and last rounds one holder's writes show in the other before the revoke, and
//!   after it nothing either writes reaches the other or the object.
//! - Each revoke's time is split three ways: the TLB shootdown, the page-table
//!   edits (with the object, space and memory locks), and the rest (the grant
//!   tree and waking calls).
//! - The timings are recorded, not judged (M6 proposes p99 at most 100 us under
//!   KVM); the case fails only if a byte crosses after a revoke.

use crate::fpu_tests::wait_exit;
use crate::handle::grant as grants;
use crate::process::{self, SpawnRequest};
use crate::{handle, hpet, ipc, log, memory, shm, smp, threads};
use alloc::string::String;
use alloc::vec::Vec;
use captable::Entry;
use core::sync::atomic::Ordering;
use kabi::handle::{Kind, Rights};
use kabi::{ExitReason, Limits, cap};
use mm::PAGE_SIZE;

const PAGES: u64 = 256;
const BYTES: u64 = PAGES * PAGE_SIZE;
const ROUNDS: usize = 50;
const HOLDERS: [&str; 2] = ["shm-a", "shm-b"];
const LIMITS: Limits = Limits {
    max_pages: 1024,
    max_burst_ms: 30_000,
    caps: cap::SERVE,
    stack_pages: 0,
};

pub fn check() -> Result<(), &'static str> {
    let mut pids = [0u64; 2];
    for (pid, name) in pids.iter_mut().zip(HOLDERS) {
        *pid = process::spawn(&SpawnRequest {
            name,
            program: "handles",
            args: b"mode=shm",
            limits: LIMITS,
            spawner: 0,
            supervisor: false,
            uses: &[],
        })
        .map_err(|_| "shm: cannot start a holder")?;
    }
    let result = rounds(pids);
    for name in HOLDERS {
        let _ = ipc::call(name, b"exit", 5_000);
    }
    for pid in pids {
        if wait_exit(pid)? != (ExitReason::Exited, 0) {
            return Err("shm: a holder failed");
        }
    }
    result?;
    shm::collect();
    log!("shm: SHM_CREATE, MAP and UNMAP work, and a revoke leaves holders sharing nothing");
    Ok(())
}

fn rounds(pids: [u64; 2]) -> Result<(), &'static str> {
    for name in HOLDERS {
        ask(name, "own", "ok")?;
    }
    let slots = pids.map(slot_of);
    let [Some(a), Some(b)] = slots else {
        return Err("shm: a holder is gone");
    };
    // Defence in depth: a shared-page edit never reaches the model window's slot.
    let refused = process::with_space(a, |space| {
        memory::with(|m| m.set_shared_page(space, kabi::layout::MODEL_BASE, None)).is_err()
    });
    if refused != Some(true) {
        return Err("shm: set_shared_page took an address in the model window's slot");
    }
    let object = shm::create_object(PAGES).map_err(|_| "shm: cannot make the object")?;
    // A plain handle in the first holder keeps the object alive across rounds.
    give(a, Entry::new(Kind::Shm, Rights::MAP | Rights::READ, object))?;
    let mut times = Vec::with_capacity(ROUNDS);
    let mut shootdowns = Vec::with_capacity(ROUNDS);
    let mut edits = Vec::with_capacity(ROUNDS);
    let mut rest = Vec::with_capacity(ROUNDS);
    for round in 0..ROUNDS {
        let node = grants::new_request(round as u64 + 1).map_err(|_| "shm: no request node")?;
        for (slot, name) in [(a, HOLDERS[0]), (b, HOLDERS[1])] {
            let entry = Entry {
                grant: Some(node),
                ..Entry::new(
                    Kind::Shm,
                    Rights::MAP | Rights::READ | Rights::WRITE,
                    object,
                )
            };
            let handle = give(slot, entry)?;
            let reply = call(name, &alloc::format!("map {handle} {PAGES}"))?;
            if !reply.starts_with("ok ") {
                log!("shm: {name} map: {reply}");
                return Err("shm: a holder could not map its handle");
            }
        }
        let checked = round == 0 || round == ROUNDS - 1;
        if checked {
            ask(HOLDERS[0], "fill 1", "ok")?;
            ask(HOLDERS[1], "count 1", &alloc::format!("ok {BYTES}"))?;
        }
        shm::SHOOTDOWN_NS.store(0, Ordering::Relaxed);
        shm::ON_REVOKE_NS.store(0, Ordering::Relaxed);
        let start = hpet::nanos();
        grants::revoke(node).map_err(|_| "shm: cannot revoke")?;
        let total = hpet::nanos() - start;
        let on_revoke = shm::ON_REVOKE_NS.load(Ordering::Relaxed).min(total);
        let shootdown = shm::SHOOTDOWN_NS.load(Ordering::Relaxed).min(on_revoke);
        times.push(total);
        shootdowns.push(shootdown);
        edits.push(on_revoke - shootdown);
        rest.push(total - on_revoke);
        if checked {
            // Both now write their own zero page: nothing reaches the other or the object.
            ask(HOLDERS[0], "count 1", "ok 0")?;
            ask(HOLDERS[0], "fill 7", "ok")?;
            ask(HOLDERS[1], "count 7", "ok 0")?;
            ask(HOLDERS[1], "fill 9", "ok")?;
            ask(HOLDERS[0], "count 9", "ok 0")?;
            let crossed = object_count(object, |byte| byte == 7 || byte == 9)?;
            let kept = object_count(object, |byte| byte == 1)?;
            if crossed != 0 || kept != BYTES {
                log!(
                    "shm: the object holds {crossed} bytes written after the revoke, {kept} of the 1s"
                );
                return Err("shm: a write through a revoked mapping reached the object");
            }
        }
        for name in HOLDERS {
            ask(name, "unmap", "ok")?;
        }
    }
    for list in [&mut times, &mut shootdowns, &mut edits, &mut rest] {
        list.sort_unstable();
    }
    let us = |list: &[u64], at: usize| list[(at * (ROUNDS - 1)) / 100] / 1000;
    log!(
        "shm: M6: revoke a {PAGES}-page Shm mapped by 2 processes on {} CPUs: p50 {} us, p99 {} us, max {} us over {ROUNDS} rounds; 0 bytes reached the other holder",
        smp::online(),
        us(&times, 50),
        us(&times, 99),
        times[ROUNDS - 1] / 1000
    );
    log!(
        "shm: M6 split: TLB shootdown p50 {} us, p99 {} us; page-table edits p50 {} us, p99 {} us; grant tree and wakes p50 {} us, p99 {} us",
        us(&shootdowns, 50),
        us(&shootdowns, 99),
        us(&edits, 50),
        us(&edits, 99),
        us(&rest, 50),
        us(&rest, 99)
    );
    Ok(())
}

fn slot_of(pid: u64) -> Option<usize> {
    process::with_table(|table| table.iter().position(|p| p.live() && p.pid == pid))
}

/// Puts `entry` in `slot`'s table; its handle number.
fn give(slot: usize, entry: Entry) -> Result<u32, &'static str> {
    handle::with(slot, |h| h.table.insert(entry))
        .ok()
        .and_then(Result::ok)
        .map(|handle| handle.0)
        .ok_or("shm: cannot give a handle")
}

/// How many of the object's bytes `matches` picks, read through the HHDM.
fn object_count(object: u64, matches: impl Fn(u8) -> bool) -> Result<u64, &'static str> {
    let frames = shm::frames_of(object).ok_or("shm: the object is gone")?;
    let hhdm = memory::hhdm();
    let mut count = 0;
    for frame in frames {
        // SAFETY: the frame is the object's, alive while the first holder keeps a
        // handle, and mapped by the HHDM; it is only read here.
        let bytes =
            unsafe { core::slice::from_raw_parts((hhdm + frame) as *const u8, PAGE_SIZE as usize) };
        count += bytes.iter().filter(|&&byte| matches(byte)).count() as u64;
    }
    Ok(count)
}

fn call(name: &str, line: &str) -> Result<String, &'static str> {
    let reply =
        ipc::call(name, line.as_bytes(), 30_000).map_err(|_| "shm: a holder did not answer")?;
    Ok(String::from_utf8_lossy(&reply).into_owned())
}

fn ask(name: &str, line: &str, want: &str) -> Result<(), &'static str> {
    let reply = call(name, line)?;
    if reply != want {
        log!("shm: {name} {line:?} -> {reply:?}, wanted {want:?}");
        return Err("shm: a holder gave the wrong answer");
    }
    threads::yield_now();
    Ok(())
}
