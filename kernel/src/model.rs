//! The model: the file on the `kai-model` disk, held in memory for the `llm`
//! service.
//!
//! At boot, before the process page budget is set, the kernel copies the disk into
//! 2 MiB runs of physically contiguous, 2 MiB-aligned frames that belong to no
//! process and are never freed, so they count against no quota and survive every
//! restart of `llm`. It checks only the GGUF magic and version; ring 3 parses
//! everything else, as untrusted input. The heap holds one u32 per run (2,560 for a
//! 5 GiB model), and everything past the file in the last run is zeroed, since
//! ring 3 can read the whole run.
//!
//! The kernel also builds, once, the page tables of the model's window (`window`):
//! a PDPT and one directory per GiB mapping each run as one 2 MiB page, read-only
//! and no-execute at every level. MODEL_MAP links that PDPT into the caller's PML4
//! slot at `kabi::layout::MODEL_BASE` with one write; teardown unlinks it before
//! freeing the process's tables, and never frees it, so a restarted llm maps the
//! model at once and takes no frame for it.

#[path = "model/progress.rs"]
mod progress;
#[path = "model/window.rs"]
pub mod window;

use crate::block::{self, Device};
use crate::{hpet, log, memory, process};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use kabi::Error;
use kabi::model::{Attached, HeaderError};
use mm::PAGE_SIZE;
use mm::dma::{SECTOR, sectors_for};
use mm::window::{RUN_BYTES, RUN_FRAMES, runs_for};
use spin::Once;

/// The one service, and program, that may map the model.
pub const HOLDER: &str = "llm";
/// Pages the llm service gets besides the engine's.
pub const HOLDER_PAGES: u64 = 1024;

/// Pages the llm service gets with a model loaded: its own, its workers' stacks
/// and the engine's KV cache and tokenizer. The model's window is the kernel's.
pub const fn holder_pages() -> u64 {
    HOLDER_PAGES + kabi::thread::POOL_PAGES + kabi::model::ENGINE_PAGES
}

// What the plan line grants llm must fit what one process may hold.
const _: () = assert!(holder_pages() <= process::MAX_PAGES_PER_PROCESS);

/// Pages the default plan's other services ask for, with room to spare: the
/// supervisor, the shell, notes and edit take 16,384 together.
const OTHER_SERVICES_PAGES: u64 = 20 * 1024;

/// Frames that must stay free after copying the model. Processes may hold half of
/// what is free, so twice what llm and the other services ask for.
const RESERVE_FRAMES: u64 = 2 * (OTHER_SERVICES_PAGES + holder_pages());

pub struct Model {
    len: u64,
    /// First frame number of each run, in file order; run `k` holds bytes
    /// `k * 2 MiB` onwards.
    runs: Vec<u32>,
}

static MODEL: Once<Model> = Once::new();
/// The window's PDPT, once built: what MODEL_MAP links.
static WINDOW: Once<window::Window> = Once::new();
/// Test only (`bigmap`): a longer window MODEL_MAP links instead.
static TEST_WINDOW: Once<window::Window> = Once::new();
/// Why an attached `kai-model` disk was not kept, for `:model`.
static REJECTED: Once<String> = Once::new();
/// Copies made from the disk, and MODEL_MAP calls that linked the window, so a test
/// can tell a restarted llm remapped the copy rather than having it made again.
static LOADS: AtomicU64 = AtomicU64::new(0);
static MAPS: AtomicU64 = AtomicU64::new(0);

/// The loaded model, if the boot found one.
pub fn get() -> Option<&'static Model> {
    MODEL.get()
}

/// How many times the model was copied from the disk: at most once a boot.
pub fn loads() -> u64 {
    LOADS.load(Ordering::SeqCst)
}

/// Why the boot did not keep the `kai-model` disk it found, if it found one.
pub fn rejected() -> Option<&'static str> {
    REJECTED.get().map(String::as_str)
}

/// How many MODEL_MAP calls linked the window.
pub fn maps() -> u64 {
    MAPS.load(Ordering::SeqCst)
}

impl Model {
    /// Length of the file in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Physical address of each run, in file order.
    pub fn runs(&self) -> impl Iterator<Item = u64> + '_ {
        self.runs.iter().map(|&frame| u64::from(frame) * PAGE_SIZE)
    }

    /// Physical address of each 4 KiB page of every run, in file order.
    pub fn frames(&self) -> impl Iterator<Item = u64> + '_ {
        self.runs()
            .flat_map(|run| (0..RUN_FRAMES).map(move |page| run + page * PAGE_SIZE))
    }

    /// The physical address of run `k`.
    pub fn run(&self, k: u64) -> u64 {
        u64::from(self.runs[k as usize]) * PAGE_SIZE
    }

    /// Runs the model takes.
    pub fn run_count(&self) -> u64 {
        self.runs.len() as u64
    }
}

/// Checks the window's tables still hold exactly what was built.
pub fn check_window() -> Result<(), &'static str> {
    let (Some(model), Some(built)) = (MODEL.get(), WINDOW.get()) else {
        return Err("model: no window to check");
    };
    window::check(built, model.run_count(), |k| model.run(k))
}

/// Copies the `kai-model` disk into memory and builds its window, if there is a
/// disk and it holds a GGUF file. Anything else is logged and ignored: the system
/// runs without a model.
pub fn load(cmdline: &str) {
    let attached = Attached::from_cmdline(cmdline);
    let started = hpet::nanos();
    match block::with_model_disk(|disk| copy(disk, attached.len)) {
        None => {}
        Some(Ok(model)) => {
            let runs = model.runs.len() as u64;
            match window::build(runs, |k| model.run(k)) {
                Ok(built) => {
                    log!(
                        "model: {} bytes of GGUF v3 copied from the kai-model disk into {runs} runs of 2 MiB in {} ms; window of {} page tables",
                        model.len,
                        (hpet::nanos() - started) / 1_000_000,
                        mm::window::table_frames(runs)
                    );
                    WINDOW.call_once(|| built);
                    MODEL.call_once(|| model);
                    LOADS.fetch_add(1, Ordering::SeqCst);
                }
                Err(why) => {
                    log!("model: {why}; the kai-model disk is ignored");
                    free_runs(&model.runs);
                    REJECTED.call_once(|| String::from(why));
                }
            }
        }
        Some(Err(why)) => {
            log!("model: {why}; the kai-model disk is ignored");
            REJECTED.call_once(|| why);
        }
    }
}

fn free_runs(runs: &[u32]) {
    memory::with(|m| {
        for &frame in runs {
            let _ = m.free_contiguous(u64::from(frame) * PAGE_SIZE, RUN_FRAMES);
        }
    });
}

fn copy(disk: &mut dyn Device, attached: Option<u64>) -> Result<Model, String> {
    let sectors = disk.sectors();
    // QEMU rounds the file up to whole sectors; model_len= says where it ends.
    let len = match attached {
        Some(len) if sectors_for(len) == sectors => len,
        Some(len) => {
            return Err(format!(
                "model_len={len} does not fit a disk of {sectors} sectors"
            ));
        }
        None => sectors.checked_mul(SECTOR).ok_or("the disk is too large")?,
    };
    if len == 0 || len > kabi::layout::MODEL_MAX {
        return Err(format!("{len} bytes is no size for a model"));
    }
    let first = disk.read(0, 1)?;
    let head = &first[..first.len().min(usize::try_from(len).unwrap_or(usize::MAX))];
    match kabi::model::check_header(head) {
        Ok(()) => {}
        Err(HeaderError::Short) => return Err(String::from("too short for a GGUF header")),
        Err(HeaderError::NoMagic) => return Err(String::from("no GGUF magic")),
        Err(HeaderError::Version(version)) => {
            return Err(format!(
                "GGUF version {version}, not {}",
                kabi::model::VERSION
            ));
        }
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
    let hhdm = memory::hhdm();
    // Where the next piece of the file goes: its offset in the file.
    let mut offset: u64 = 0;
    let mut progress = progress::Progress::new(len);
    let copied = block::stream(disk, len, |_, bytes| {
        let started = hpet::nanos();
        // Reads are 512 KiB and start on a page boundary, so a piece never crosses a
        // run: four of them fill one.
        if offset.is_multiple_of(RUN_BYTES) {
            let phys = memory::with(|m| m.allocate_aligned(RUN_FRAMES, RUN_FRAMES))
                .ok_or("no 2 MiB run of free memory")?;
            let Ok(frame) = u32::try_from(phys / PAGE_SIZE) else {
                memory::with(|m| m.free_contiguous(phys, RUN_FRAMES)).ok();
                return Err("a run above 16 TiB");
            };
            list.push(frame);
        }
        let run = u64::from(*list.last().ok_or("no run")?) * PAGE_SIZE;
        let at = offset % RUN_BYTES;
        if at + bytes.len() as u64 > RUN_BYTES {
            return Err("a disk read crossed a 2 MiB run");
        }
        // SAFETY: the run was allocated for the model alone and the direct map covers
        // it; the copy stays inside it.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (hhdm + run + at) as *mut u8,
                bytes.len(),
            )
        };
        offset += bytes.len() as u64;
        progress.piece(offset, hpet::nanos() - started);
        Ok(())
    });
    if let Err(why) = copied {
        free_runs(&list);
        return Err(String::from(why));
    }
    if list.len() != count || offset != len {
        free_runs(&list);
        return Err(String::from("the disk did not yield the whole file"));
    }
    // Ring 3 can read the whole last run: no stale memory may follow the file.
    let last = u64::from(list[count - 1]) * PAGE_SIZE;
    let tail = len - (runs - 1) * RUN_BYTES;
    // SAFETY: as above; the zeroing stays inside the last run.
    unsafe {
        core::ptr::write_bytes(
            (hhdm + last + tail) as *mut u8,
            0,
            (RUN_BYTES - tail) as usize,
        )
    };
    Ok(Model { len, runs: list })
}

/// Whether the process in `slot` may map the model: the `llm` service running the
/// `llm` program, which only the supervisor's grant rule lets hold `cap::MODEL`, or
/// a process the kernel started itself with that capability (its tests). The
/// dispatcher checked the capability.
fn may_map(slot: usize) -> bool {
    let me = process::identity(slot);
    (me.name.as_str() == HOLDER && me.program.as_str() == HOLDER) || me.spawner == 0
}

/// The MODEL_MAP call: links the model's window into the calling process and
/// returns the model's length. Takes no frame; calling it again changes nothing.
pub fn map(slot: usize) -> Result<u64, Error> {
    if !may_map(slot) {
        return Err(Error::Permission);
    }
    let model = MODEL.get().ok_or(Error::NotFound)?;
    let pdpt = TEST_WINDOW
        .get()
        .or(WINDOW.get())
        .ok_or(Error::NotFound)?
        .pdpt;
    process::link_window(slot, kabi::layout::MODEL_BASE, pdpt)?;
    MAPS.fetch_add(1, Ordering::SeqCst);
    Ok(model.len)
}

/// Test only (`bigmap`): builds a window of `runs` runs, run `k` at physical
/// `source(k)`, with the same builder as the model's, and has MODEL_MAP link it
/// from now on. Once a boot.
pub fn use_test_window(runs: u64, source: impl Fn(u64) -> u64) -> Result<(), &'static str> {
    if TEST_WINDOW.get().is_some() {
        return Err("model: the test window is built already");
    }
    let built = window::build(runs, source)?;
    TEST_WINDOW.call_once(|| built);
    Ok(())
}

/// Test only: checks the test window's tables still hold exactly what was built.
pub fn check_test_window(runs: u64, source: impl Fn(u64) -> u64) -> Result<(), &'static str> {
    let built = TEST_WINDOW.get().ok_or("model: no test window")?;
    window::check(built, runs, source)
}
