//! L6 self-tests for worker threads, with `workers` processes the kernel starts
//! itself (no supervisor).
//!
//! `test=threads`: `mode=basic` (a worker pinned on every CPU, the refusals, the
//! call filter, then `exit` with workers still running) must exit 0; a worker that
//! overflows its stack kills its process with a page fault (vector 14); a process
//! that exits right after starting workers ends cleanly, and the next process,
//! which may take its slot at once, is not disturbed; a process whose threads all
//! sleep an hour at a time ends at once when the kernel kills it (kill wakes every
//! thread); a process without `cap::THREADS` is refused. Every worker started on
//! the CPU it was pinned to. With a 500 ms burst limit (L6's per-thread
//! accounting): a worker computing forever kills its process as a runaway; a worker
//! on every CPU and the main thread each computing 300 ms without blocking, far
//! more than 500 ms together, all live, though main starts at once on tids the
//! runaway may have left; and a service computing 1,500 ms for a kernel call answers
//! it, then is killed once it computes with no call to serve. Afterwards no user
//! frame is left and every thread slot the processes took is free again. Then
//! `mode=futex`: ping-pong through FUTEX_WAIT and FUTEX_WAKE with a worker on
//! every CPU, and the calls' answers for a stale value, a timeout, no waiter, a
//! misaligned word and unmapped or kernel memory.
//!
//! `test=threads-kill`: the kernel kills a process whose main thread and workers
//! all wait on futexes with no timeout, then one whose workers compute in ring 3;
//! each ends Killed, after which USER_FRAMES is 0, every thread slot is free, and
//! the free frames are exactly what they were before the process, counting the
//! kernel stacks a new thread slot keeps. The futex ping-pong then passes again
//! in a new process, likely in the same slot.

use crate::fpu_tests::wait_exit;
use crate::memory::stack;
use crate::memory::user::USER_FRAMES;
use crate::process::{self, SpawnRequest};
use crate::{hpet, ipc, log, memory, smp, threads};
use core::sync::atomic::Ordering;
use kabi::{ExitReason, Limits, cap};

/// Pages each test process may hold: its image, heap, stacks and their tables.
const PAGES: u64 = 2048;
/// How long thread slots may take to be released once the processes are gone.
const SETTLE_MS: u64 = 5_000;
/// The burst limit of the burst steps, and what their threads compute.
const BURST_MS: u64 = 500;
/// The serve step's request: three times the limit, for a caller that waits 20 s.
const SERVE_REQUEST: &[u8] = b"spin 1500";
const SERVE_WAIT_MS: u64 = 20_000;

/// Starts `workers` with `args` and `caps`, as the kernel's own child.
pub(crate) fn start(name: &str, args: &str, caps: u64, burst_ms: u64) -> Result<u64, &'static str> {
    process::spawn(&SpawnRequest {
        name,
        program: "workers",
        args: args.as_bytes(),
        limits: Limits {
            max_pages: PAGES,
            max_burst_ms: burst_ms,
            caps,
            stack_pages: 0,
        },
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|error| {
        log!("threads: cannot start {name}: {error:?}");
        "threads: cannot start a workers process"
    })
}

/// Runs one `workers` process to its end and checks how it ended.
fn expect(name: &str, args: &str, caps: u64, want: (ExitReason, u64)) -> Result<(), &'static str> {
    expect_burst(name, args, caps, 10_000, want)
}

/// [`expect`] with a burst limit of `burst_ms`.
fn expect_burst(
    name: &str,
    args: &str,
    caps: u64,
    burst_ms: u64,
    want: (ExitReason, u64),
) -> Result<(), &'static str> {
    let pid = start(name, args, caps, burst_ms)?;
    let got = wait_exit(pid)?;
    log!(
        "threads: {name} ({args}) ended: {} ({})",
        got.0.name(),
        got.1
    );
    if got != want {
        return Err("threads: a workers process ended the wrong way");
    }
    Ok(())
}

/// Waits until the thread table is back to `slots` threads in use.
pub(crate) fn settle(slots: usize) -> Result<(), &'static str> {
    let deadline = hpet::nanos() + SETTLE_MS * 1_000_000;
    while threads::in_use() != slots {
        if hpet::nanos() > deadline {
            log!(
                "threads: {} thread slots in use, {slots} before",
                threads::in_use()
            );
            return Err("threads: thread slots were not released");
        }
        threads::sleep_ms(10);
    }
    Ok(())
}

/// Kills a process whose every thread sleeps an hour at a time: only `kill`
/// waking each of them ends it.
fn kill_sleepers(caps: u64) -> Result<(), &'static str> {
    let count = smp::online() + 1;
    let pid = start("sleepers", "mode=sleep", caps, 10_000)?;
    let deadline = hpet::nanos() + SETTLE_MS * 1_000_000;
    while process::thread_count(pid) < count {
        if hpet::nanos() > deadline {
            return Err("threads: the sleeping workers did not start");
        }
        threads::sleep_ms(10);
    }
    threads::sleep_ms(50);
    let killed_at = hpet::nanos();
    process::kill(pid, ExitReason::Killed).map_err(|_| "threads: the sleepers were gone")?;
    let got = wait_exit(pid)?;
    log!(
        "threads: killed {count} sleeping threads; the process ended in {} ms",
        (hpet::nanos() - killed_at) / 1_000_000
    );
    if got != (ExitReason::Killed, 0) {
        return Err("threads: the sleepers ended the wrong way");
    }
    Ok(())
}

/// The compute allowance: a service computes for longer than its limit while it
/// serves the kernel's call, answers, and is a runaway once it computes for nobody.
fn serve_check() -> Result<(), &'static str> {
    let pid = start("compute", "mode=serve", cap::SERVE, BURST_MS)?;
    let reply = ipc::call("compute", SERVE_REQUEST, SERVE_WAIT_MS).map_err(|error| {
        log!("threads: the compute service did not answer: {error:?}");
        "threads: a service computing for a call was not let finish"
    })?;
    if reply != b"ok spun" {
        return Err("threads: the compute service answered something else");
    }
    let got = wait_exit(pid)?;
    log!(
        "threads: the compute service answered after 1500 ms with a {BURST_MS} ms limit, then ended: {} ({})",
        got.0.name(),
        got.1
    );
    if got != (ExitReason::Runaway, BURST_MS) {
        return Err("threads: computing with no call to serve was not a runaway");
    }
    Ok(())
}

/// How a `threads-kill` process waits for its kill.
#[derive(Clone, Copy)]
enum Waiting {
    /// Main and every worker are in futex waits with no timeout.
    Futex,
    /// Main waits on a futex; the workers compute in ring 3.
    Compute,
}

/// Starts `mode`, waits until every thread is in place, kills it, and checks it
/// ended Killed with every frame and thread slot back.
fn kill_mid_run(mode: &str, waiting: Waiting) -> Result<(), &'static str> {
    let slots = threads::in_use();
    let cpus = smp::online();
    let before = memory::with(|m| m.free_frames());
    let stacks = stack::frames_taken();
    let pid = start(mode, &alloc::format!("mode={mode}"), cap::THREADS, 0)?;
    let deadline = hpet::nanos() + SETTLE_MS * 1_000_000;
    let ready = || match waiting {
        Waiting::Futex => process::futex::waiters(pid) == cpus + 1,
        Waiting::Compute => {
            process::thread_count(pid) == cpus + 1 && process::futex::waiters(pid) == 1
        }
    };
    while !ready() {
        if hpet::nanos() > deadline {
            return Err("threads-kill: the threads did not get in place");
        }
        threads::sleep_ms(10);
    }
    // Let computing workers run in ring 3 with their vector state live.
    threads::sleep_ms(100);
    process::kill(pid, ExitReason::Killed).map_err(|_| "threads-kill: the process was gone")?;
    if wait_exit(pid)? != (ExitReason::Killed, 0) {
        return Err("threads-kill: the process ended the wrong way");
    }
    settle(slots)?;
    let after = memory::with(|m| m.free_frames());
    let new_stacks = stack::frames_taken() - stacks;
    let user_frames = USER_FRAMES.load(Ordering::SeqCst);
    log!(
        "threads-kill: {mode}: {} threads killed; free frames {before} before, {after} after, {new_stacks} taken by new kernel stacks; USER_FRAMES {user_frames}",
        cpus + 1
    );
    if user_frames != 0 || after + new_stacks != before {
        return Err("threads-kill: frames were not all freed");
    }
    Ok(())
}

/// How long `threads-kill` waits for the network to get its address.
const NET_WAIT_MS: u64 = 15_000;

/// What the `thread-pin` mover thread saw, for the case to read.
static PIN_STAYED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static PIN_MIGRATED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static PIN_DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static SPIN_STOP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Rounds a pinned thread must stay put through.
const PIN_ROUNDS: usize = 200;

/// The `thread-pin` case (THREAD_PIN, for llm's main thread): a kernel thread
/// pinned to CPU 1 never runs elsewhere through yields and sleeps; unpinned
/// while CPU 1 is crowded, load balancing moves it off. Then THREAD_PIN from
/// ring 3, and refused without cap::THREADS.
pub fn pin_check() -> Result<(), &'static str> {
    if smp::online() < 2 {
        return Err("thread-pin: needs at least 2 CPUs");
    }
    threads::spawn("pin-test", pin_mover, 0).map_err(|_| "thread-pin: cannot start the mover")?;
    let deadline = hpet::nanos() + 30_000 * 1_000_000;
    while !PIN_DONE.load(Ordering::SeqCst) {
        if hpet::nanos() > deadline {
            return Err("thread-pin: the mover never finished");
        }
        threads::sleep_ms(10);
    }
    SPIN_STOP.store(true, Ordering::SeqCst);
    if !PIN_STAYED.load(Ordering::SeqCst) {
        return Err("thread-pin: a pinned thread ran on another CPU");
    }
    if !PIN_MIGRATED.load(Ordering::SeqCst) {
        return Err("thread-pin: an unpinned thread never moved off a crowded CPU");
    }
    expect("pin", "mode=pin", cap::THREADS, (ExitReason::Exited, 0))?;
    log!(
        "thread-pin: a pinned thread stayed on its CPU, an unpinned one migrated, and ring 3 can pin"
    );
    Ok(())
}

fn pin_mover(_: usize) {
    let pinned = threads::pin_current(Some(1)).is_ok();
    let mut stayed = pinned;
    for round in 0..PIN_ROUNDS {
        if round % 2 == 0 {
            threads::yield_now();
        } else {
            threads::sleep_ms(1);
        }
        stayed &= crate::percpu::index() == 1 && threads::owner(threads::current_tid()) == 1;
    }
    PIN_STAYED.store(stayed, Ordering::SeqCst);
    // Crowd CPU 1 with pinned spinners, then unpin: balancing pulls this thread off.
    for _ in 0..3 {
        let _ = threads::spawn_pinned(1, "pin-spin", pin_spinner, 0);
    }
    let _ = threads::pin_current(None);
    let deadline = hpet::nanos() + 5_000 * 1_000_000;
    while hpet::nanos() < deadline {
        if crate::percpu::index() != 1 {
            PIN_MIGRATED.store(true, Ordering::SeqCst);
            break;
        }
        threads::yield_now();
    }
    SPIN_STOP.store(true, Ordering::SeqCst);
    PIN_DONE.store(true, Ordering::SeqCst);
    threads::exit();
}

fn pin_spinner(_: usize) {
    while !SPIN_STOP.load(Ordering::SeqCst) {
        core::hint::spin_loop();
    }
    threads::exit();
}

/// The `threads-kill` case.
pub fn kill_check() -> Result<(), &'static str> {
    // The free-frame count is the whole machine's: DHCP, set going at boot,
    // allocates when its lease arrives, and once landed between the two counts
    // and failed the case one frame short. So the network settles first: an
    // address from DHCP or the fallback, or no card at all.
    let settled = || crate::net::status().is_none_or(|status| status.address.is_some());
    let deadline = hpet::nanos() + NET_WAIT_MS * 1_000_000;
    while !settled() {
        if hpet::nanos() > deadline {
            return Err("threads-kill: the network never got an address");
        }
        threads::sleep_ms(10);
    }
    kill_mid_run("park", Waiting::Futex)?;
    kill_mid_run("spin", Waiting::Compute)?;
    expect(
        "pingpong",
        "mode=futex",
        cap::THREADS,
        (ExitReason::Exited, 0),
    )?;
    if process::workers::misplaced() != 0 {
        return Err("threads-kill: a worker started on another CPU than its own");
    }
    log!("threads-kill: every thread woke for its kill, and every frame came back");
    Ok(())
}

/// The `threads` case.
pub fn check() -> Result<(), &'static str> {
    let slots = threads::in_use();
    let caps = cap::SERVE | cap::THREADS;
    expect("threads", "mode=basic", caps, (ExitReason::Exited, 0))?;
    expect("overflow", "mode=overflow", caps, (ExitReason::Crashed, 14))?;
    expect("spawnexit", "mode=spawnexit", caps, (ExitReason::Exited, 0))?;
    kill_sleepers(caps)?;
    expect("nocap", "mode=nocap", cap::SERVE, (ExitReason::Exited, 0))?;
    let runaway = (ExitReason::Runaway, BURST_MS);
    expect_burst("runaway", "mode=runaway", caps, BURST_MS, runaway)?;
    let lived = (ExitReason::Exited, 0);
    expect_burst("burst", "mode=burst", caps, BURST_MS, lived)?;
    serve_check()?;
    expect("futex", "mode=futex", caps, (ExitReason::Exited, 0))?;
    settle(slots)?;
    let user_frames = USER_FRAMES.load(Ordering::SeqCst);
    log!(
        "threads: workers ran on {} CPUs; USER_FRAMES {user_frames}, {} thread slots in use as before",
        smp::online(),
        slots
    );
    if user_frames != 0 {
        return Err("threads: user frames leaked");
    }
    if process::workers::misplaced() != 0 {
        return Err("threads: a worker started on another CPU than its own");
    }
    Ok(())
}
