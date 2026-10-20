//! `mode=bigmap` and `mode=nomap`: reading the model window above 4 GiB.
//!
//! For the `bigmap` case the kernel makes the window `window_runs` runs of 2 MiB
//! long by aliasing: the first `model_runs` runs are the model's, the next
//! `high_runs` are copies of model runs placed above 4 GiB of physical memory, and
//! the pattern repeats. Run `k` must therefore hold what model run `source(k)`
//! holds, which the program checks at both ends and in the middle of every run of
//! the window, by reading the low runs, which are the model itself.

use kabi::Error;
use kabi::layout::MODEL_BASE;
use user::{arg, println, sys};

const RUN: u64 = 2 << 20;
/// Offsets checked in every run: its first and last bytes, page edges, the middle.
const OFFSETS: [u64; 6] = [0, 8, 4088, 4096, RUN / 2 + 24, RUN - 8];

/// The model run that window run `k` copies.
fn source(k: u64, model_runs: u64, high_runs: u64) -> u64 {
    let cycle = k % (model_runs + high_runs);
    if cycle < model_runs {
        cycle
    } else {
        (cycle - model_runs) % model_runs
    }
}

fn read(offset: u64) -> u64 {
    // SAFETY: MODEL_MAP mapped the window read-only at MODEL_BASE, and every offset
    // read here lies inside it (checked against `window_runs` by the caller).
    unsafe { core::ptr::read_volatile((MODEL_BASE + offset) as *const u64) }
}

pub fn bigmap(args: &str) -> i32 {
    let number = |key: &str| arg(args, key).and_then(|v| v.parse::<u64>().ok());
    let (Some(window_runs), Some(model_runs), Some(high_runs)) = (
        number("window_runs"),
        number("model_runs"),
        number("high_runs"),
    ) else {
        println!("workers: bigmap needs window_runs=, model_runs= and high_runs=");
        return 140;
    };
    if model_runs == 0 || window_runs == 0 {
        return 141;
    }
    let len = match sys::model_map() {
        Ok(len) => len,
        Err(error) => {
            println!("workers: MODEL_MAP: {error:?}");
            return 142;
        }
    };
    if len.div_ceil(RUN) != model_runs {
        println!("workers: a model of {len} bytes is not {model_runs} runs");
        return 143;
    }
    let mut above_4g = 0;
    for k in 0..window_runs {
        let from = source(k, model_runs, high_runs);
        for offset in OFFSETS {
            let (here, there) = (read(k * RUN + offset), read(from * RUN + offset));
            if here != there {
                println!(
                    "workers: window run {k} (copy of run {from}) differs at {offset:#x}: {here:#x}, not {there:#x}"
                );
                return 144;
            }
        }
        if k * RUN >= 4 << 30 {
            above_4g += 1;
        }
    }
    // A second MODEL_MAP in the same process changes nothing.
    if sys::model_map() != Ok(len) {
        return 145;
    }
    println!(
        "workers: bigmap read {window_runs} runs of the window, {above_4g} of them past 4 GiB, each equal to the model run it aliases"
    );
    0
}

/// `mode=nomap`: started without `cap::MODEL`, MODEL_MAP must say Permission.
pub fn nomap() -> i32 {
    match sys::model_map() {
        Err(Error::Permission) => 0,
        other => {
            println!("workers: MODEL_MAP without cap::MODEL gave {other:?}");
            146
        }
    }
}
