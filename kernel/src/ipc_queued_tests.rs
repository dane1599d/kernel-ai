//! #1164's review: a call queued on a server that ends is woken at once, so
//! APP_CTL's `gone` cancel fires rather than the 5 s deadline, and APP_CTL
//! answers Dead for a ctl that was taken (it may have taken effect). That
//! cancel takes no lock in the call's `ready()` ([`ready_rule`]).
//!
//! Holder `hqheld` takes call A and holds it 3 s. Call B, from here, queues
//! behind A with APP_CTL's cancel (the server is no longer `hqheld`'s pid).
//! Holder `hqother` holds a call of its own, and call C queues behind it.
//! `hqheld` is killed: B ends Cancelled at once, A ends ServiceDied, and C,
//! on an endpoint the dead process never bound, is still waiting and was not
//! woken (its cancel check, run each time it wakes, ran no more after the kill).

use crate::fpu_tests::wait_exit;
use crate::process::syscall::standing::ctl_cancel;
use crate::process::{self, SpawnRequest};
use crate::{hpet, ipc, log, threads};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use kabi::{Error, ExitReason, Limits, cap};

const HELD: &str = "hqheld";
const OTHER: &str = "hqother";
/// Deadlines far past the holders' 3 s, so only a wake ends the calls early.
const CALL_MS: u64 = 10_000;
/// B must end this soon after the kill: well inside the holders' 3 s.
const WOKEN_MS: u64 = 1_000;
/// How long after B is queued the holder is killed.
const KILL_AFTER_MS: u64 = 300;

const LIMITS: Limits = Limits {
    max_pages: 256,
    max_burst_ms: 10_000,
    caps: cap::SERVE,
    stack_pages: 0,
};

/// A's outcome: 0 while running, then [`outcome`]'s code.
static A_DONE: AtomicU8 = AtomicU8::new(0);
static C_DONE: AtomicBool = AtomicBool::new(false);
/// How many times C's cancel check ran: once each time C's thread wakes.
static C_WAKES: AtomicU64 = AtomicU64::new(0);
/// C_WAKES just before the kill, and [`SETTLE_MS`] after it.
static C_WAKES_AT_KILL: AtomicU64 = AtomicU64::new(0);
static C_WAKES_AFTER: AtomicU64 = AtomicU64::new(u64::MAX);
static HELD_PID: AtomicU64 = AtomicU64::new(0);
/// How long after the kill C's wakes are counted again: past B's wake.
const SETTLE_MS: u64 = 200;

const SERVICE_DIED: u8 = 1;
const OTHER_END: u8 = 2;

fn outcome(result: &Result<(alloc::vec::Vec<u8>, u64), ipc::CallError>) -> u8 {
    match result {
        Err(ipc::CallError::ServiceDied) => SERVICE_DIED,
        _ => OTHER_END,
    }
}

fn holder(name: &'static str) -> Result<u64, &'static str> {
    let pid = process::spawn(&SpawnRequest {
        name,
        program: "handles",
        args: b"mode=hold",
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|_| "ipc-queued: cannot start a holder")?;
    for _ in 0..1000 {
        if ipc::server_of(name).is_some() {
            return Ok(pid);
        }
        threads::sleep_ms(10);
    }
    Err("ipc-queued: a holder never bound")
}

fn call_a(_: usize) {
    let result = ipc::call_from(HELD, b"hold", CALL_MS);
    A_DONE.store(outcome(&result), Ordering::SeqCst);
}

fn hold_other(_: usize) {
    let _ = ipc::call_from(OTHER, b"hold", CALL_MS);
}

fn call_c(_: usize) {
    let counted = || {
        C_WAKES.fetch_add(1, Ordering::SeqCst);
        false
    };
    let _ = ipc::call_from_unless(OTHER, b"queued", CALL_MS, counted);
    C_DONE.store(true, Ordering::SeqCst);
}

fn killer(_: usize) {
    threads::sleep_ms(KILL_AFTER_MS);
    C_WAKES_AT_KILL.store(C_WAKES.load(Ordering::SeqCst), Ordering::SeqCst);
    let _ = process::kill(HELD_PID.load(Ordering::SeqCst), ExitReason::Killed);
    threads::sleep_ms(SETTLE_MS);
    C_WAKES_AFTER.store(C_WAKES.load(Ordering::SeqCst), Ordering::SeqCst);
}

pub fn check() -> Result<(), &'static str> {
    ctl_mapping()?;
    let held = holder(HELD)?;
    let other = holder(OTHER)?;
    HELD_PID.store(held, Ordering::SeqCst);
    let spawn = |name, entry| threads::spawn(name, entry, 0).map(|_| ());
    spawn("ipcq-a", call_a).map_err(|_| "ipc-queued: no thread for A")?;
    spawn("ipcq-o", hold_other).map_err(|_| "ipc-queued: no thread for the other hold")?;
    // A and the other hold are taken before B and C queue behind them.
    let mut taken = false;
    for _ in 0..500 {
        taken = ipc::serving_deadline(held).is_some() && ipc::serving_deadline(other).is_some();
        if taken {
            break;
        }
        threads::sleep_ms(10);
    }
    if !taken {
        return Err("ipc-queued: the holders never took A and the other hold");
    }
    spawn("ipcq-c", call_c).map_err(|_| "ipc-queued: no thread for C")?;
    spawn("ipcq-k", killer).map_err(|_| "ipc-queued: no killer thread")?;
    ready_rule()?;
    // APP_CTL's own cancel.
    let gone = ctl_cancel(HELD).ok_or("ipc-queued: the held server is not bound")?;
    let queued_at = hpet::nanos();
    let b = ipc::call_from_unless(HELD, b"queued", CALL_MS, gone);
    let took_ms = hpet::nanos().saturating_sub(queued_at) / 1_000_000;
    let c_waiting = !C_DONE.load(Ordering::SeqCst);
    if wait_exit(held)?.0 != ExitReason::Killed {
        return Err("ipc-queued: the held server was not killed");
    }
    for _ in 0..100 {
        if A_DONE.load(Ordering::SeqCst) != 0 {
            break;
        }
        threads::sleep_ms(20);
    }
    // The killer's count after the kill, before `other` lets C go.
    for _ in 0..100 {
        if C_WAKES_AFTER.load(Ordering::SeqCst) != u64::MAX {
            break;
        }
        threads::sleep_ms(20);
    }
    let after = C_WAKES_AFTER.load(Ordering::SeqCst);
    let counted = after != u64::MAX;
    let c_woken = counted && after != C_WAKES_AT_KILL.load(Ordering::SeqCst);
    let _ = wait_exit(other)?;
    if !matches!(b, Err(ipc::CallError::Cancelled)) {
        return Err("ipc-queued: a call queued on a server that died did not end Cancelled");
    }
    if took_ms > KILL_AFTER_MS + WOKEN_MS {
        log!("ipc-queued: the queued call ended {took_ms} ms after it was made");
        return Err("ipc-queued: a call queued on a server that died waited on");
    }
    if A_DONE.load(Ordering::SeqCst) != SERVICE_DIED {
        return Err("ipc-queued: the call the dead server held did not end ServiceDied");
    }
    if !counted {
        return Err("ipc-queued: the killer never counted C's wakes after the kill");
    }
    if !c_waiting || c_woken {
        return Err("ipc-queued: a call queued on another server was woken too");
    }
    log!(
        "ipc-queued: a call queued on a server that died ended Cancelled in {took_ms} ms; the held one ServiceDied; another server's queue was left alone"
    );
    Ok(())
}

/// A call's cancel runs in its sleep's `ready()`, where a lock held with
/// interrupts on can freeze two CPUs (`threads::ready`). Debug builds count such
/// a lock: `server_of`, APP_CTL's cancel before, takes the IPC lock and is
/// counted; APP_CTL's cancel now takes none. Every boot case also fails when a
/// real `ready()` took one (`main::run_case`).
fn ready_rule() -> Result<(), &'static str> {
    let looked_up = threads::ready::locks_in_ready(|| {
        let _ = ipc::server_of(HELD);
    });
    if cfg!(debug_assertions) && looked_up == 0 {
        return Err("ipc-queued: a lock taken in a ready() was not counted");
    }
    let gone = ctl_cancel(HELD).ok_or("ipc-queued: the held server is not bound")?;
    if threads::ready::locks_in_ready(|| {
        let _ = gone();
    }) != 0
    {
        return Err("ipc-queued: APP_CTL's cancel takes a lock in a ready()");
    }
    Ok(())
}

/// APP_CTL's answers: taken and not answered, or the app gone holding it, is
/// Dead (it may have taken effect); never taken is TimedOut or NotFound.
fn ctl_mapping() -> Result<(), &'static str> {
    use crate::process::syscall::standing::ctl_error;
    use ipc::CallError as C;
    let cases = [
        (C::Unanswered, Error::Dead),
        (C::ServiceDied, Error::Dead),
        (C::TimedOut, Error::TimedOut),
        (C::Cancelled, Error::TimedOut),
        (C::Busy, Error::TimedOut),
        (C::Unavailable, Error::NotFound),
        (C::NoMemory, Error::NoMemory),
    ];
    if cases
        .iter()
        .any(|&(call, answer)| ctl_error(call) != answer)
    {
        return Err("ipc-queued: APP_CTL does not answer Dead for exactly a taken ctl");
    }
    Ok(())
}
