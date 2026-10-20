//! `test=bigmap` (L6, the judges' graft): the model window past 4 GiB, without a
//! multi-GB disk. The kernel builds a second window, test only, with the same
//! builder as the model's: 2,561 runs (5 GiB and 2 MiB) whose run `k` is the
//! `k mod (n + h)`th of the model's `n` runs followed by `h` copies of model runs
//! placed above 4 GiB of physical memory. A kernel-started `workers` process maps it
//! and checks that every window run holds the bytes of the model run it aliases, at
//! offsets on both sides of 4 GiB.
//!
//! Exact counts: building the window takes exactly its directories and PDPT; each
//! reader process, once gone, leaves the free frames exactly as before it (with the
//! kernel stacks a new thread slot keeps counted), USER_FRAMES is 0, and the
//! window's tables are intact; each reader maps it twice, the second time changing
//! nothing. A reader without
//! `cap::MODEL` is refused first.

use crate::fpu_tests::wait_exit;
use crate::memory::stack;
use crate::memory::user::USER_FRAMES;
use crate::thread_tests::{settle, start};
use crate::{log, memory, model, threads};
use alloc::format;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use kabi::{ExitReason, cap};
use mm::PAGE_SIZE;
use mm::window::{RUN_BYTES, RUN_FRAMES, runs_for, table_frames};

/// The window's length: past 5 GiB, so its offsets cross 4 GiB and use six
/// directories.
const WINDOW_BYTES: u64 = (5 << 30) + RUN_BYTES;
/// Copies of model runs placed above 4 GiB of physical memory.
const HIGH_RUNS: u64 = 3;
const FOUR_GIB: u64 = 4 << 30;
/// Reader processes: the first may take new kernel stacks, the others none.
const READERS: usize = 3;

pub fn check() -> Result<(), &'static str> {
    let model = model::get().ok_or("bigmap: the kai-model disk was not loaded")?;
    let n = model.run_count();
    // Without the capability, a kernel-started process is refused.
    let slots = threads::in_use();
    let pid = start("nomap", "mode=nomap", 0, 10_000)?;
    if wait_exit(pid)? != (ExitReason::Exited, 0) {
        return Err("bigmap: MODEL_MAP without cap::MODEL was not refused");
    }
    settle(slots)?;
    let high = copies_above_4g(model)?;
    let window_runs = runs_for(WINDOW_BYTES);
    let source = |k: u64| -> u64 {
        let cycle = k % (n + HIGH_RUNS);
        if cycle < n {
            model.run(cycle)
        } else {
            high[(cycle - n) as usize]
        }
    };
    let free_before = memory::with(|m| m.free_frames());
    model::use_test_window(window_runs, source)?;
    let taken = free_before
        .checked_sub(memory::with(|m| m.free_frames()))
        .ok_or("bigmap: frames were freed while the window was built")?;
    log!(
        "bigmap: window of {window_runs} runs ({} MiB) built in {taken} frames; {n} model runs and {HIGH_RUNS} copies above 4 GiB at {:#x}",
        window_runs * 2,
        high[0]
    );
    if taken != table_frames(window_runs) {
        return Err("bigmap: building the window took more than its tables");
    }
    let maps = model::maps();
    let args =
        format!("mode=bigmap window_runs={window_runs} model_runs={n} high_runs={HIGH_RUNS}");
    for reader in 0..READERS {
        let slots = threads::in_use();
        let before = memory::with(|m| m.free_frames());
        let stacks = stack::frames_taken();
        let pid = start("bigmap", &args, cap::MODEL, 10_000)?;
        if wait_exit(pid)? != (ExitReason::Exited, 0) {
            return Err("bigmap: the reader found a run that is not what it aliases");
        }
        settle(slots)?;
        let after = memory::with(|m| m.free_frames());
        let new_stacks = stack::frames_taken().saturating_sub(stacks);
        log!(
            "bigmap: reader {reader}: free frames {before} before, {after} after, {new_stacks} taken by new kernel stacks; USER_FRAMES {}",
            USER_FRAMES.load(Ordering::SeqCst)
        );
        if after + new_stacks != before || USER_FRAMES.load(Ordering::SeqCst) != 0 {
            return Err("bigmap: a reader did not give back every frame");
        }
    }
    model::check_test_window(window_runs, source)?;
    // Each reader maps twice: the second call must change nothing, and counts too.
    if model::maps() - maps != 2 * READERS as u64 || model::loads() != 1 {
        return Err("bigmap: the window was not mapped twice per reader");
    }
    log!(
        "bigmap: {READERS} readers read past 4 GiB of the window; every frame came back, the tables intact"
    );
    Ok(())
}

/// Allocates `HIGH_RUNS` runs at or above 4 GiB physical and copies model runs
/// into them.
fn copies_above_4g(model: &model::Model) -> Result<Vec<u64>, &'static str> {
    let hhdm = memory::hhdm();
    let mut high = Vec::new();
    for j in 0..HIGH_RUNS {
        let run = memory::with(|m| m.allocate_aligned_from(FOUR_GIB, RUN_FRAMES, RUN_FRAMES))
            .ok_or("bigmap: no free 2 MiB run above 4 GiB (give the VM more memory)")?;
        let from = model.run(j % model.run_count());
        // SAFETY: both runs are 2 MiB of RAM in the direct map: `from` belongs to the
        // model and is only read, `run` was just allocated for this test alone.
        unsafe {
            core::ptr::copy_nonoverlapping(
                (hhdm + from) as *const u8,
                (hhdm + run) as *mut u8,
                RUN_BYTES as usize,
            )
        };
        high.push(run);
    }
    debug_assert!(
        high.iter()
            .all(|&run| run >= FOUR_GIB && run % (PAGE_SIZE * RUN_FRAMES) == 0)
    );
    Ok(high)
}
