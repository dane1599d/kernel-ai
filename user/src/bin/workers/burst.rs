//! `mode=burst`, `mode=runaway` and `mode=serve`: per-thread burst accounting and
//! the compute allowance. The kernel starts these with a 500 ms burst limit. Every
//! stretch of compute runs until the clock says so, never a fixed amount of work,
//! so what the kernel charges does not depend on how fast the CPU is emulated.

use core::sync::atomic::{AtomicUsize, Ordering};
use kabi::thread::MAX_WORKERS;
use user::{println, sys};

/// Each thread's stretch in `mode=burst`: under the limit alone, past it together.
const STRETCH_MS: u64 = 300;
const STACK: u64 = 4;

static DONE: AtomicUsize = AtomicUsize::new(0);

/// Computes without blocking until `ms` have passed on the clock.
fn spin_ms(ms: u64) -> u64 {
    let deadline = sys::time_ns() + ms * 1_000_000;
    let mut x: u64 = 1;
    while sys::time_ns() < deadline {
        for _ in 0..10_000 {
            x = core::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
        }
    }
    x
}

extern "C" fn stretch(_: u64) -> ! {
    spin_ms(STRETCH_MS);
    DONE.fetch_add(1, Ordering::AcqRel);
    let refused = sys::thread_exit();
    println!("workers: THREAD_EXIT failed: {refused:?}");
    sys::exit(90)
}

extern "C" fn endless(_: u64) -> ! {
    let mut x: u64 = 1;
    loop {
        x = core::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
    }
}

/// `mode=burst`: a worker on every CPU and the main thread each compute 300 ms
/// without blocking, more than the 500 ms limit together. Main computes at once,
/// without sleeping first, so a tid left over from an earlier runaway would show.
pub fn burst() -> i32 {
    let cpus = sys::metrics()
        .map_or(1, |m| m.cpus as usize)
        .clamp(1, MAX_WORKERS);
    for cpu in 0..cpus {
        if sys::thread_spawn(stretch, 0, cpu, STACK).is_err() {
            return 60;
        }
    }
    spin_ms(STRETCH_MS);
    let deadline = sys::time_ns() + 30_000_000_000;
    while DONE.load(Ordering::Acquire) < cpus {
        if sys::time_ns() > deadline {
            return 61;
        }
        sys::sleep_ms(1);
    }
    println!(
        "workers: {} threads computed {STRETCH_MS} ms each and lived",
        cpus + 1
    );
    0
}

/// `mode=runaway`: a worker computes forever; the kernel must kill the process.
pub fn runaway() -> i32 {
    if sys::thread_spawn(endless, 0, 0, STACK).is_err() {
        return 70;
    }
    for _ in 0..3_000 {
        sys::sleep_ms(10);
    }
    71
}

/// `mode=serve`: answers one `spin MS` request after computing that long, then
/// computes with no call to serve, which the kernel must kill.
pub fn serve() -> i32 {
    // Sleeping first restarts the burst that loading the program used.
    sys::sleep_ms(1);
    let mut request = [0u8; 64];
    let mut caller = 0;
    let Ok(len) = sys::svc_recv(&mut request, 0, &mut caller) else {
        return 80;
    };
    let text = core::str::from_utf8(&request[..len.min(request.len())]).unwrap_or("");
    let Some(ms) = text
        .strip_prefix("spin ")
        .and_then(|ms| ms.trim().parse::<u64>().ok())
    else {
        return 81;
    };
    spin_ms(ms);
    if sys::svc_reply(b"ok spun").is_err() {
        return 82;
    }
    spin_ms(60_000);
    83
}
