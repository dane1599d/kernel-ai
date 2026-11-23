//! `test=model-store fill=restart` and `fill=giveup` (#1163's review): MODEL_FILL's
//! restart and give-up paths, driven with the real block runs of the store's
//! file. Just before `fs`'s first MODEL_FILL goes through, [`before_fill`] uses
//! its extents to make the case:
//!
//! - **restart**: a one-block fill, then a call at an offset that is neither
//!   where it stopped nor 0, which is refused with the model still pending;
//!   `fs`'s own call at offset 0 then starts the fill over, and `store` checks
//!   the published model's FNV-1a64 is the hint's.
//! - **giveup**: a one-block fill, then a run past the partition's end: the
//!   call is Invalid, the model is no longer pending, MODEL_MAP answers
//!   NotFound (rejected, nothing published), and the reserved runs are free
//!   again. `fs`'s own call then finds nothing pending.

use crate::block::{self, Device};
use crate::model::{self, fill};
use crate::{hpet, log, memory, threads};
use core::sync::atomic::{AtomicBool, Ordering};
use kabi::Error;
use mm::window::{RUN_FRAMES, runs_for};
use spin::Mutex;

/// How long `store` waits for the hook's verdict.
const VERDICT_WAIT_MS: u64 = 60_000;
/// Frames other work may take or give back while the give-up runs.
const FRAME_SLACK: u64 = 128;

static RAN: AtomicBool = AtomicBool::new(false);
static VERDICT: Mutex<Option<Result<(), &'static str>>> = Mutex::new(None);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Case {
    Restart,
    GiveUp,
}

/// The `fill=` word on this test boot's command line.
pub fn case() -> Option<Case> {
    if !crate::test_boot() {
        return None;
    }
    crate::cmdline()
        .split_whitespace()
        .find_map(|word| match word {
            "fill=restart" => Some(Case::Restart),
            "fill=giveup" => Some(Case::GiveUp),
            _ => None,
        })
}

/// From MODEL_FILL, before `fs`'s call is filled: the case, once, on `fs`'s
/// first call (offset 0, with runs).
pub fn before_fill(extents: &[(u64, u64)], offset: u64) {
    let Some(case) = case() else {
        return;
    };
    let Some(&(first, _)) = extents.first() else {
        return;
    };
    if offset != 0 || RAN.swap(true, Ordering::SeqCst) {
        return;
    }
    let verdict = match case {
        Case::Restart => wrong_offset(first),
        Case::GiveUp => give_up(first),
    };
    *VERDICT.lock() = Some(verdict);
}

/// One block filled, then a call at neither that offset nor 0.
fn wrong_offset(first: u64) -> Result<(), &'static str> {
    let filled = fill::fill(&[(first, 1)], 0).map_err(|_| "model-fill: a one-block fill failed")?;
    if filled != fill::BLOCK {
        return Err("model-fill: a one-block fill did not fill one block");
    }
    if fill::fill(&[(first + 1, 1)], filled + 1) != Err(Error::Invalid) {
        return Err("model-fill: a fill at a wrong offset was not refused");
    }
    if !fill::pending() || model::rejected().is_some() {
        return Err("model-fill: a refused offset ended the wait for the model");
    }
    log!("model-fill: a fill at a wrong offset is refused, and the model is still pending");
    Ok(())
}

/// One block filled, then a run past the partition's end.
fn give_up(first: u64) -> Result<(), &'static str> {
    let len = fill::hint().ok_or("model-fill: no kai-modelhint")?.len;
    fill::fill(&[(first, 1)], 0).map_err(|_| "model-fill: a one-block fill failed")?;
    let sectors = block::with_partition(diskmap::KAI_DATA_TYPE, |disk: &mut dyn Device| {
        disk.sectors()
    })
    .ok_or("model-fill: no kai-data")?;
    let past_end = sectors.div_ceil(fill::BLOCK / 512) + 1;
    let before = memory::with(|m| m.free_frames());
    if fill::fill(&[(past_end, 1)], fill::BLOCK) != Err(Error::Invalid) {
        return Err("model-fill: a run past kai-data's end was not Invalid");
    }
    let freed = memory::with(|m| m.free_frames()).saturating_sub(before);
    if fill::pending() {
        return Err("model-fill: a failed fill left the model pending");
    }
    if model::rejected().is_none() || model::get().is_some() {
        return Err("model-fill: a failed fill did not end in NotFound");
    }
    let runs = runs_for(len) * RUN_FRAMES;
    if freed + FRAME_SLACK < runs {
        log!("model-fill: {freed} frames came back of the {runs} reserved");
        return Err("model-fill: a failed fill kept the reserved runs");
    }
    log!(
        "model-fill: a failed fill gives up: not pending, MODEL_MAP NotFound, {freed} of {runs} frames back"
    );
    Ok(())
}

/// `store`'s wait for [`before_fill`]'s verdict.
pub fn verdict() -> Result<(), &'static str> {
    let start = hpet::nanos();
    loop {
        if let Some(verdict) = *VERDICT.lock() {
            return verdict;
        }
        if hpet::nanos().saturating_sub(start) > VERDICT_WAIT_MS * 1_000_000 {
            return Err("model-fill: fs never called MODEL_FILL");
        }
        threads::sleep_ms(50);
    }
}
