//! The model from the model store (P2 task 10, ADR 0004 §8): when the boot has
//! neither a `kai.model` boot module nor a `kai-model` disk, the model is the
//! file `/system/models/<sha256>.gguf` in kaifs on `kai-data`.
//!
//! - **At boot** the kernel reads `kai-data`'s sector 0, which kaifs leaves out.
//!   A `kai-modelhint 1` record there ([`kabi::modelhint`]) gives the model's
//!   length, so the kernel reserves its 2 MiB runs before the process page
//!   budget is set, as [`super::copy`] does for a disk. The model stays
//!   unpublished: MODEL_MAP answers Busy.
//! - **MODEL_FILL**: once `fs` has mounted, it hands the kernel the file's block
//!   runs in `kai-data`, in file order. The kernel reads each run itself, checked
//!   against the partition's bounds, into the reserved runs, hashing as it goes.
//!   At the end it checks the GGUF header and the hint's FNV-1a64, zeroes the
//!   tail, and publishes the model; on any mismatch it frees the runs and the
//!   system runs without a model, as with a bad disk.
//!
//! `fs` cannot make the kernel read outside `kai-data`, nor publish anything but
//! bytes whose hash the hint, written with the import, names.

use super::{Model, RESERVE_FRAMES, free_runs};
use crate::block::{self, Device};
use crate::{log, memory};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::Error;
use kabi::model::Fnv1a;
use kabi::modelhint::Hint;
use mm::PAGE_SIZE;
use mm::window::{RUN_BYTES, RUN_FRAMES, runs_for};
use spin::Mutex;

/// Bytes in a kaifs block, the unit of the runs `fs` hands over.
pub const BLOCK: u64 = 4096;
/// The most runs one MODEL_FILL takes.
pub const MAX_EXTENTS: usize = 64;

/// A model reserved at boot and not yet filled.
struct Pending {
    hint: Hint,
    runs: Vec<u32>,
    /// Bytes filled so far, from the file's start.
    filled: u64,
    fnv: Fnv1a,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// Whether a model is reserved and waiting for MODEL_FILL.
pub fn pending() -> bool {
    PENDING.lock().is_some()
}

/// The `kai-modelhint 1` record on `kai-data`, if the disk has one that verifies.
pub fn hint() -> Option<Hint> {
    block::with_partition(diskmap::KAI_DATA_TYPE, |disk: &mut dyn Device| {
        disk.read(0, 1).ok().and_then(Hint::decode)
    })
    .flatten()
}

/// Reserves the runs for the model `hint` names, or says why not.
pub fn reserve(hint: Hint) -> Result<(), String> {
    let len = hint.len;
    if len > kabi::layout::MODEL_MAX {
        return Err(format!("{len} bytes is no size for a model"));
    }
    let runs = runs_for(len);
    let needed = runs * RUN_FRAMES + mm::window::table_frames(runs);
    let free = memory::with(|m| m.free_frames());
    if needed.saturating_add(RESERVE_FRAMES) > free {
        return Err(format!(
            "{} MiB do not fit in {} MiB of free memory with {} MiB left for the services",
            needed / 256,
            free / 256,
            RESERVE_FRAMES / 256
        ));
    }
    let count = usize::try_from(runs).map_err(|_| "too many runs")?;
    let mut list: Vec<u32> = Vec::new();
    list.try_reserve_exact(count)
        .map_err(|_| "no kernel heap for the run list")?;
    for _ in 0..count {
        let Some(phys) = memory::with(|m| m.allocate_aligned(RUN_FRAMES, RUN_FRAMES)) else {
            free_runs(&list);
            return Err(String::from("no 2 MiB run of free memory"));
        };
        let Ok(frame) = u32::try_from(phys / PAGE_SIZE) else {
            memory::with(|m| m.free_contiguous(phys, RUN_FRAMES)).ok();
            free_runs(&list);
            return Err(String::from("a run above 16 TiB"));
        };
        list.push(frame);
    }
    *PENDING.lock() = Some(Pending {
        hint,
        runs: list,
        filled: 0,
        fnv: Fnv1a::new(),
    });
    Ok(())
}

/// MODEL_FILL: copies the file's bytes from `offset` on, out of the `kai-data`
/// block runs `extents` (first block, blocks) in file order, into the reserved
/// runs. `offset` must be where the last call stopped. Returns the bytes filled
/// in all; at the model's length the model is checked and published.
///
/// No extents ends the wait without a model: `fs` found the hinted file missing
/// or short, so the runs are freed and MODEL_MAP answers NotFound from then on,
/// and llm's wait ends.
pub fn fill(extents: &[(u64, u64)], offset: u64) -> Result<u64, Error> {
    if extents.len() > MAX_EXTENTS {
        return Err(Error::Invalid);
    }
    let mut guard = PENDING.lock();
    let pending = guard.as_mut().ok_or(Error::NotFound)?;
    if extents.is_empty() {
        let given_up = guard.take().expect("pending");
        drop(guard);
        free_runs(&given_up.runs);
        log!("model: fs has no file for the hinted model; the boot runs without one");
        super::reject(String::from(
            "the model store does not hold the hinted model",
        ));
        return Ok(0);
    }
    if offset != pending.filled {
        return Err(Error::Invalid);
    }
    let hhdm = memory::hhdm();
    let len = pending.hint.len;
    for &(first, blocks) in extents {
        let sector = first.checked_mul(BLOCK / 512).ok_or(Error::Invalid)?;
        let sectors = blocks.checked_mul(BLOCK / 512).ok_or(Error::Invalid)?;
        let copied = block::with_partition(diskmap::KAI_DATA_TYPE, |disk: &mut dyn Device| {
            let end = sector.checked_add(sectors).ok_or(Error::Invalid)?;
            if sectors == 0 || end > disk.sectors() {
                return Err(Error::Invalid);
            }
            // Runs of at most 512 KiB, page-aligned in the file, so a piece
            // never crosses a 2 MiB run.
            let step = disk.max_sectors().clamp(8, 1024) / 8 * 8;
            let mut at = sector;
            while at < end && pending.filled < len {
                let n = step.min(end - at);
                let bytes = disk.read(at, n).map_err(|_| Error::Unavailable)?;
                let take = (bytes.len() as u64).min(len - pending.filled) as usize;
                let run = pending.filled / RUN_BYTES;
                let inside = pending.filled % RUN_BYTES;
                if inside + take as u64 > RUN_BYTES {
                    return Err(Error::Invalid);
                }
                let phys = u64::from(pending.runs[run as usize]) * PAGE_SIZE;
                // SAFETY: the run is the model's alone and in the direct map;
                // the copy stays inside it (checked above).
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        (hhdm + phys + inside) as *mut u8,
                        take,
                    )
                };
                pending.fnv.update(&bytes[..take]);
                pending.filled += take as u64;
                at += n;
            }
            Ok(())
        });
        copied.ok_or(Error::NotFound)??;
    }
    let filled = pending.filled;
    if filled == len {
        let done = guard.take().expect("pending");
        drop(guard);
        finish(done)?;
    }
    Ok(filled)
}

/// Checks the filled model and publishes it, or frees it.
fn finish(done: Pending) -> Result<(), Error> {
    let hhdm = memory::hhdm();
    let first = u64::from(done.runs[0]) * PAGE_SIZE;
    let head_len = done.hint.len.min(512) as usize;
    // SAFETY: run 0 is the model's, in the direct map, at least `head_len` long.
    let head = unsafe { core::slice::from_raw_parts((hhdm + first) as *const u8, head_len) };
    let header = kabi::model::check_header(head).is_ok();
    let hash = done.fnv.finish() == done.hint.fnv;
    if !header || !hash {
        let why = match header {
            false => "no GGUF v3 header",
            true => "its FNV-1a64 is not the hint's",
        };
        log!("model: the model store's file is refused: {why}");
        free_runs(&done.runs);
        super::reject(String::from(why));
        return Err(Error::Invalid);
    }
    let runs = done.runs.len() as u64;
    let last = u64::from(done.runs[runs as usize - 1]) * PAGE_SIZE;
    let tail = done.hint.len - (runs - 1) * RUN_BYTES;
    // SAFETY: as above; the zeroing stays inside the last run.
    unsafe {
        core::ptr::write_bytes(
            (hhdm + last + tail) as *mut u8,
            0,
            (RUN_BYTES - tail) as usize,
        )
    };
    let model = Model {
        len: done.hint.len,
        runs: done.runs,
    };
    super::publish(model, "the model store", Some(done.hint.sha256)).map_err(|_| Error::NoMemory)
}
