//! `test=ipc-rtt`: IPC round trips, timed. `handles mode=echo` serves, answering
//! each request with its own bytes; for 0 B, 1 KiB and 7 KiB requests:
//! - the kernel calls it ([`ipc::call`]), kernel to ring 3;
//! - `handles mode=rtt` calls it on a route, ring 3 to ring 3, and prints its
//!   own figures (`user/src/bin/handles/rtt.rs`).
//!
//! For the kernel's calls, each leg is timed too ([`ipc::legs`]): queued to
//! taken (the server's wake), taken to answered (the server's work and its
//! system calls), and answered to back (the caller's wake), with how many
//! calls had the server on the caller's CPU.
//!
//! Each size's p50 and p99 is logged. They are recorded, not judged: the case
//! fails only if a call fails or an echo comes back another size, and the
//! figures are read from the log, as spike S01-1's M1 is.

use crate::fpu_tests::wait_exit;
use crate::process::{self, SpawnRequest};
use crate::{hpet, ipc, log, threads};
use alloc::vec::Vec;
use kabi::{ExitReason, Limits, cap};

/// The echo server's name, which the client's route names.
const ECHO: &str = "rtt-echo";
/// The sizes timed, as the ring-3 client times them.
const SIZES: [usize; 3] = [0, 1024, 7 * 1024];
/// Calls made before the timing starts.
const WARMUP: usize = 50;
/// Calls timed for each size.
const ROUNDS: usize = 1000;
/// How long one call may take.
const CALL_MS: u64 = 10_000;
/// How long the echo server may take to start serving.
const START_MS: u64 = 10_000;

/// Room for a 7 KiB message and its replies.
const LIMITS: Limits = Limits {
    max_pages: 256,
    max_burst_ms: 30_000,
    caps: cap::SERVE,
    stack_pages: 0,
};

/// The `p`th percentile of `times`, which it sorts.
fn percentile(times: &mut [u64], p: usize) -> u64 {
    times.sort_unstable();
    let at = (times.len() * p / 100).min(times.len().saturating_sub(1));
    times.get(at).copied().unwrap_or(0)
}

/// Starts the echo server and waits until it serves.
fn start_echo() -> Result<(), &'static str> {
    let request = SpawnRequest {
        name: ECHO,
        program: "handles",
        args: b"mode=echo",
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    };
    process::spawn_with(&request, Vec::new(), false)
        .map_err(|_| "ipc-rtt: cannot start the echo server")?;
    for _ in 0..START_MS / 10 {
        if ipc::server_of(ECHO).is_some() {
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    Err("ipc-rtt: the echo server never served")
}

/// Each size's round trip from the kernel.
fn from_kernel() -> Result<(), &'static str> {
    for size in SIZES {
        let request = alloc::vec![b'k'; size];
        let call = || -> Result<(), &'static str> {
            let reply = ipc::call(ECHO, &request, CALL_MS)
                .map_err(|_| "ipc-rtt: a call from the kernel failed")?;
            match reply.len() == size {
                true => Ok(()),
                false => Err("ipc-rtt: the echo came back another size"),
            }
        };
        for _ in 0..WARMUP {
            call()?;
        }
        let mut times: Vec<u64> = Vec::with_capacity(ROUNDS);
        let mut legs: [Vec<u64>; 3] = core::array::from_fn(|_| Vec::with_capacity(ROUNDS));
        let mut same_cpu = 0;
        for _ in 0..ROUNDS {
            let start = hpet::nanos();
            call()?;
            let back = hpet::nanos();
            times.push(back.saturating_sub(start));
            let ([queued, taken, replied], cpus) = ipc::legs::last();
            legs[0].push(taken.saturating_sub(queued));
            legs[1].push(replied.saturating_sub(taken));
            legs[2].push(back.saturating_sub(replied));
            if cpus[0] == cpus[1] {
                same_cpu += 1;
            }
        }
        let p99 = percentile(&mut times, 99);
        let p50 = percentile(&mut times, 50);
        log!("ipc-rtt: kernel to ring 3, {size} B: p50 {p50} ns, p99 {p99} ns over {ROUNDS} calls");
        let [wake, work, back] = legs.map(|mut leg| percentile(&mut leg, 50));
        log!(
            "ipc-rtt: kernel to ring 3, {size} B, p50 legs: server's wake {wake} ns, its work {work} ns, caller's wake {back} ns; server on the caller's CPU in {same_cpu} of {ROUNDS}"
        );
    }
    Ok(())
}

/// The ring-3 client's run, which ends the echo server with `exit`.
fn from_ring3() -> Result<(), &'static str> {
    let request = SpawnRequest {
        name: "rtt-client",
        program: "handles",
        args: b"mode=rtt",
        limits: Limits {
            caps: cap::CALL,
            ..LIMITS
        },
        spawner: 0,
        supervisor: false,
        uses: &[ECHO],
    };
    let pid = process::spawn_with(&request, Vec::new(), false)
        .map_err(|_| "ipc-rtt: cannot start the client")?;
    match wait_exit(pid)? {
        (ExitReason::Exited, 0) => Ok(()),
        _ => Err("ipc-rtt: the ring-3 client failed"),
    }
}

pub fn check() -> Result<(), &'static str> {
    start_echo()?;
    let handoffs = threads::handoffs();
    ipc::legs::on(true);
    let timed = from_kernel();
    ipc::legs::on(false);
    timed?;
    log!(
        "ipc-rtt: {} of the kernel's calls pulled the echo server onto the caller's CPU",
        threads::handoffs() - handoffs
    );
    from_ring3()?;
    log!("ipc-rtt: the echo server answered each size from the kernel and from ring 3");
    Ok(())
}
