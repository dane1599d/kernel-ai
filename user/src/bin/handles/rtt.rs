//! `test=ipc-rtt`'s two ring-3 ends: `mode=echo` serves, answering each
//! request with its own bytes until `exit`; `mode=rtt` calls it on its first
//! route with 0 B, 1 KiB and 7 KiB requests and prints each size's round trip,
//! p50 and p99, from ring 3 to ring 3. The kernel's side times its own calls to
//! the same server (`kernel/src/ipc_rtt_tests.rs`).

use alloc::vec::Vec;
use kabi::handle::{Handle, RecvInfo};
use user::{println, sys};

/// What ends the echo server.
pub const EXIT: &[u8] = b"exit";
/// The request sizes timed: an empty message, 1 KiB, and 7 KiB, which stays
/// under kabi's 8 KiB `MAX_MESSAGE` with room for a header.
pub const SIZES: [usize; 3] = [0, 1024, 7 * 1024];
/// Calls made before the timing starts, so caches and first faults are behind.
const WARMUP: usize = 50;
/// Calls timed for each size.
const ROUNDS: usize = 1000;
/// How long one call may take before the client gives up.
const CALL_MS: u64 = 10_000;
/// How long the server waits for its next request.
const IDLE_MS: u64 = 60_000;

/// `mode=echo`: each request answered with its own bytes, until `exit`.
pub fn echo() -> Result<(), &'static str> {
    let mut buf = alloc::vec![0u8; kabi::MAX_MESSAGE];
    loop {
        let mut info = RecvInfo::default();
        let len = sys::recv_h(&mut buf, IDLE_MS, &mut info).map_err(|_| "echo: recv_h failed")?;
        let request = &buf[..len.min(buf.len())];
        if request == EXIT {
            sys::reply_h(b"bye", &[]).map_err(|_| "echo: the last reply failed")?;
            return Ok(());
        }
        sys::reply_h(request, &[]).map_err(|_| "echo: reply_h failed")?;
    }
}

/// The `p`th percentile of `times`, which it sorts.
pub fn percentile(times: &mut [u64], p: usize) -> u64 {
    times.sort_unstable();
    let at = (times.len() * p / 100).min(times.len().saturating_sub(1));
    times.get(at).copied().unwrap_or(0)
}

/// `mode=rtt`: each size's round trip to the echo server on route 0.
pub fn rtt() -> Result<(), &'static str> {
    let route = user::route(0).ok_or("rtt: no route to the echo server")?;
    let mut reply = alloc::vec![0u8; kabi::MAX_MESSAGE];
    let mut none: [Handle; 0] = [];
    for size in SIZES {
        let request = alloc::vec![b'k'; size];
        let mut call = |reply: &mut [u8]| -> Result<(), &'static str> {
            let (len, _) = sys::call_h(route, &request, reply, CALL_MS, &[], &mut none)
                .map_err(|_| "rtt: a call failed")?;
            if len != size {
                return Err("rtt: the echo came back another size");
            }
            Ok(())
        };
        for _ in 0..WARMUP {
            call(&mut reply)?;
        }
        let mut times: Vec<u64> = Vec::with_capacity(ROUNDS);
        for _ in 0..ROUNDS {
            let start = sys::time_ns();
            call(&mut reply)?;
            times.push(sys::time_ns().saturating_sub(start));
        }
        let p99 = percentile(&mut times, 99);
        let p50 = percentile(&mut times, 50);
        println!(
            "ipc-rtt: ring 3 to ring 3, {size} B: p50 {p50} ns, p99 {p99} ns over {ROUNDS} calls"
        );
    }
    let (len, _) = sys::call_h(route, EXIT, &mut reply, CALL_MS, &[], &mut none)
        .map_err(|_| "rtt: the exit call failed")?;
    if &reply[..len] != b"bye" {
        return Err("rtt: the echo server did not say bye");
    }
    Ok(())
}
