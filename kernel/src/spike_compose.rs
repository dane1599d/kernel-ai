//! Spike S05-1 "compose and look" (ADR 0005), kernel half; the `spikes` feature
//! only, never merged.
//!
//! - SPIKE_DISPLAY_MAP maps the display's back buffer into the caller in 4 KiB
//!   pages and stops the console drawing, as DISPLAY_CLAIM will hand it out as a
//!   kernel-owned `Shm`.
//! - SPIKE_DISPLAY_FLUSH flushes up to 32 rectangles, as DISPLAY_FLUSH will.
//! - SPIKE_SHARE_MAP maps one of four shared buffers into every process that
//!   asks; SPIKE_BELL_WAIT and SPIKE_BELL_RING (`process::futex`) are the
//!   cross-process wake: a futex keyed by the word's frame.
//! - `test=compose` runs the ring-3 program `compose`, which renders the scenes
//!   and prints its own figures, then the compositor-and-client pair for scene
//!   (e), then holds each screendump state while xtask takes it.

use crate::display;
use crate::process::{self, SpawnRequest, supervision, uaccess};
use crate::{console, log, memory, threads};
use alloc::vec::Vec;
use gfxcore::Rect;
use kabi::layout::{SPIKE_DISPLAY_BASE, SPIKE_SHARE_BASE, SPIKE_SHARE_SLOT};
use kabi::{Error, ExitReason, Limits};
use mm::PAGE_SIZE;
use spin::Mutex;

const MAX_RECTS: usize = 32;
const SHARES: usize = 4;

/// Shared buffer `id`'s frames, allocated by its first mapper and never freed.
static SHARED: Mutex<[Option<Vec<u64>>; SHARES]> = Mutex::new([const { None }; SHARES]);

fn map_frames(slot: usize, base: u64, frames: impl Iterator<Item = u64>) -> Result<(), Error> {
    let mut space = process::lock_space(slot);
    let space = space.as_mut().ok_or(Error::Dead)?;
    memory::with(|m| {
        for (n, phys) in frames.enumerate() {
            m.map_shared_page(space, base + n as u64 * PAGE_SIZE, phys, true)
                .map_err(|_| Error::NoMemory)?;
        }
        Ok(())
    })
}

/// SPIKE_DISPLAY_MAP.
pub fn display_map(slot: usize, info: u64) -> Result<u64, Error> {
    let surface = display::surface().ok_or(Error::NotFound)?;
    let bytes = (surface.stride * surface.height * 4) as u64;
    let phys = surface.base.as_ptr() as u64 - memory::hhdm();
    console::detach();
    map_frames(
        slot,
        SPIKE_DISPLAY_BASE,
        (0..bytes.div_ceil(PAGE_SIZE)).map(|n| phys + n * PAGE_SIZE),
    )?;
    let words = [
        surface.width as u64,
        surface.height as u64,
        surface.stride as u64,
    ];
    let mut out = [0u8; 24];
    for (chunk, word) in out.as_chunks_mut::<8>().0.iter_mut().zip(words) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    uaccess::copy_to_user(slot, info, &out)?;
    Ok(SPIKE_DISPLAY_BASE)
}

/// SPIKE_DISPLAY_FLUSH.
pub fn display_flush(slot: usize, rects: u64, n: u64) -> Result<u64, Error> {
    let n = usize::try_from(n).map_err(|_| Error::Invalid)?;
    if n > MAX_RECTS {
        return Err(Error::Invalid);
    }
    let mut raw = [0u8; MAX_RECTS * 16];
    uaccess::copy_from_user(slot, rects, &mut raw[..n * 16])?;
    for r in raw[..n * 16].as_chunks::<16>().0 {
        let words = r.as_chunks::<4>().0;
        let word = |i: usize| u32::from_le_bytes(words[i]);
        display::flush(Rect::new(word(0), word(1), word(2), word(3))).map_err(|_| Error::Busy)?;
    }
    Ok(0)
}

/// SPIKE_SHARE_MAP.
pub fn share_map(slot: usize, id: u64, bytes: u64) -> Result<u64, Error> {
    let id = usize::try_from(id)
        .ok()
        .filter(|&id| id < SHARES)
        .ok_or(Error::Invalid)?;
    if bytes == 0 || bytes > SPIKE_SHARE_SLOT {
        return Err(Error::Invalid);
    }
    let frames = {
        let mut shared = SHARED.lock();
        if shared[id].is_none() {
            let mut frames = Vec::new();
            for _ in 0..bytes.div_ceil(PAGE_SIZE) {
                frames.push(memory::with(|m| m.allocate_zeroed_frame()).ok_or(Error::NoMemory)?);
            }
            shared[id] = Some(frames);
        }
        shared[id].clone().unwrap_or_default()
    };
    let base = SPIKE_SHARE_BASE + id as u64 * SPIKE_SHARE_SLOT;
    map_frames(slot, base, frames.into_iter())?;
    Ok(base)
}

/// The compose program: room for a 4K frame, its atlas and the fonts, and long
/// bursts, since a scene renders 300 frames back to back.
const LIMITS: Limits = Limits {
    max_pages: 96 * 1024,
    max_burst_ms: 60_000,
    caps: 0,
    stack_pages: 64,
};
const RUN_TIMEOUT_MS: u64 = 600_000;

/// The hogs of scene (f): M10's `work` hogs, CPU-bound, never blocking.
const HOG_LIMITS: Limits = Limits {
    max_pages: 1024,
    max_burst_ms: 600_000,
    caps: 0,
    stack_pages: 0,
};

fn start(name: &str, args: &str) -> Result<u64, &'static str> {
    spawn(name, "compose", args, LIMITS)
}

fn spawn(name: &str, program: &str, args: &str, limits: Limits) -> Result<u64, &'static str> {
    process::spawn(&SpawnRequest {
        name,
        program,
        args: args.as_bytes(),
        limits,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|error| {
        log!("compose: cannot start {name}: {error:?}");
        "compose: cannot start the spike program"
    })
}

fn wait(pid: u64) -> Result<(), &'static str> {
    for _ in 0..RUN_TIMEOUT_MS / 10 {
        match supervision::kernel_child_exit(pid) {
            Some((ExitReason::Exited, 0)) => return Ok(()),
            Some(exit) => {
                log!("compose: process {pid} ended {exit:?}");
                return Err("compose: a spike process failed");
            }
            None => threads::sleep_ms(10),
        }
    }
    Err("compose: a spike process did not end")
}

/// Scene (f): scene (b) again with one `work` hog per CPU (mix B's hogs) under
/// `classes`, the hogs labelled background.
fn under_load() -> Result<(), &'static str> {
    let cpus = crate::smp::online().clamp(1, 6);
    let mut hogs = Vec::new();
    for i in 1..=cpus {
        hogs.push(spawn(
            &alloc::format!("hog-{i}"),
            "work",
            &alloc::format!("mix=B index={i} seed=1"),
            HOG_LIMITS,
        )?);
    }
    let classes = sched::spec::PolicySpec::new(
        sched::spec::PolicyKind::Classes,
        sched::spec::Preset::Gentle,
    );
    crate::sched_control::install(classes, hogs.clone())
        .map_err(|_| "compose: cannot install classes")?;
    log!("compose: (f) {cpus} hogs under classes");
    threads::sleep_ms(500);
    let result = wait(start("compose", "role=drag")?);
    for pid in hogs {
        let _ = process::kill(pid, ExitReason::Killed);
    }
    let _ = crate::sched_control::set_policy(sched::spec::PolicySpec::DEFAULT);
    result
}

/// `test=compose`: the scenes, (f) under load, then scene (e)'s pair.
pub fn check() -> Result<(), &'static str> {
    wait(start("compose", "role=scenes")?)?;
    under_load()?;
    let compositor = start("compositor", "role=compositor")?;
    let client = start("client", "role=client")?;
    wait(client)?;
    wait(compositor)?;
    log!("compose: done");
    Ok(())
}
