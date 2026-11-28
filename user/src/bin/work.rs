//! M10's workload: one process of a `sched::workload` mix, as ring-3 code. The
//! arguments `mix=A index=I seed=N` pick the process and its seed; it draws every
//! length from the same streams as the simulator, so `editor`, `build-1` and the
//! rest behave in the OS as they do on the host.
//!
//! - A hog computes forever (its plan line gives it a CPU limit of an hour).
//! - A wakeful process computes for a drawn time and sleeps for a drawn time.
//! - A probe (the editor) waits for events at seeded times, like key presses, and
//!   handles each with a short burst; events it is late for queue.
//!
//! "Compute for N ns" means a calibrated count of loop iterations, not a wall-clock
//! wait, so a burst needs the same CPU however often it is preempted. Calibration
//! takes the fastest of many timed runs, each a fraction of a tick, so at least
//! one ran unpreempted even while every process of the mix calibrates at once. The
//! program prints one line at start and nothing while it runs, so measurement
//! windows stay quiet.

#![no_std]
#![no_main]

use sched::sim::{Behavior, Rng};
use sched::workload::{self, Mix};
use user::{arg, args, println, sys};

user::entry!(main);

const MS: u64 = 1_000_000;
fn main() -> i32 {
    let args = args();
    let mix = arg(&args, "mix").and_then(Mix::by_name);
    let index = arg(&args, "index").and_then(|i| i.parse::<usize>().ok());
    let seed = arg(&args, "seed").and_then(|s| s.parse::<u64>().ok());
    let (Some(mix), Some(index), Some(seed)) = (mix, index, seed) else {
        println!("expects mix=A|B index=I seed=N, got: {args}");
        return 2;
    };
    let Some(proc) = mix.procs.get(index) else {
        println!("mix {} has no process {index}", mix.name);
        return 2;
    };
    let Some(speed) = calibrate() else {
        println!(
            "the clock timed no run of up to {} iterations at {} ns or more; not calibrated",
            workload::MAX_CALIBRATION_ITERS,
            workload::MIN_CALIBRATION_NS
        );
        return 3;
    };
    println!(
        "up: mix {} process {index} ({}), seed {seed}, {} iterations per ms",
        mix.name, proc.name, speed
    );
    let mut work = Rng::stream(seed, index, 1);
    let mut timing = Rng::stream(seed, index, 2);
    match proc.behavior {
        Behavior::Hog => loop {
            spin(u64::MAX / 2);
        },
        Behavior::Wakeful { run, sleep } => loop {
            compute(run.draw(&mut work), speed);
            sys::sleep_ms(sleep.draw(&mut timing).div_ceil(MS));
        },
        Behavior::Probe { gap, burst } => {
            let mut next = sys::time_ns() + gap.draw(&mut timing);
            loop {
                let now = sys::time_ns();
                if now < next {
                    sys::sleep_ms((next - now).div_ceil(MS));
                }
                compute(burst.draw(&mut work), speed);
                next += gap.draw(&mut timing);
            }
        }
    }
}

/// Loop iterations per millisecond ([`workload::calibrate`]), each run timed
/// with `time_ns`.
fn calibrate() -> Option<u64> {
    workload::calibrate(|iterations| {
        let start = sys::time_ns();
        spin(iterations);
        sys::time_ns().saturating_sub(start)
    })
}

/// Computes for about `ns` of CPU time.
fn compute(ns: u64, per_ms: u64) {
    spin(ns.saturating_mul(per_ms) / MS);
}

fn spin(iterations: u64) {
    let mut counter = 0u64;
    for _ in 0..iterations {
        counter = core::hint::black_box(counter.wrapping_add(1));
    }
}
