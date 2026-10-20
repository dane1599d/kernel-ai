//! The model window's page tables in memory: built once from zeroed frames with
//! `mm::window`'s host-tested encoder, every entry of every table written whole, and
//! checked by reading them back against where they were built. The frames are the
//! kernel's and never freed.

use crate::memory;
use alloc::vec::Vec;
use mm::window::{self, directories_for};

/// A built window: its PDPT, which MODEL_MAP links, and its directories.
pub struct Window {
    pub pdpt: u64,
    directories: Vec<u64>,
}

/// Builds the tables of a window of `runs` runs, run `k` at physical `run(k)`.
pub fn build(runs: u64, run: impl Fn(u64) -> u64) -> Result<Window, &'static str> {
    let count = directories_for(runs);
    if runs == 0 || runs > window::MAX_RUNS {
        return Err("a model window must be 1 to 262,144 runs");
    }
    let mut directories = Vec::new();
    directories
        .try_reserve_exact(count as usize)
        .map_err(|_| "no kernel heap for the window's tables")?;
    let tables = memory::with(|m| {
        let pdpt = m.allocate_zeroed_frame()?;
        for _ in 0..count {
            match m.allocate_zeroed_frame() {
                Some(directory) => directories.push(directory),
                None => {
                    for &directory in &directories {
                        let _ = m.free_frame(directory);
                    }
                    let _ = m.free_frame(pdpt);
                    return None;
                }
            }
        }
        Some(pdpt)
    });
    let pdpt = tables.ok_or("out of frames for the window's tables")?;
    for (d, &directory) in directories.iter().enumerate() {
        let entries =
            window::directory(d as u64, runs, &run).map_err(|_| "a run is not 2 MiB-aligned")?;
        write(directory, &entries);
    }
    let entries = window::pdpt(&directories).map_err(|_| "a window table is misplaced")?;
    write(pdpt, &entries);
    let built = Window { pdpt, directories };
    check(&built, runs, &run)?;
    Ok(built)
}

/// Reads the window's tables back and compares every entry with the encoder's for
/// the frames it was built in, ignoring the Accessed and Dirty bits the CPU sets,
/// and checks every one of those frames is still allocated: a table freed while
/// still linked would otherwise go unnoticed until its frame was reused.
pub fn check(built: &Window, runs: u64, run: impl Fn(u64) -> u64) -> Result<(), &'static str> {
    let directories = &built.directories;
    if directories.len() as u64 != directories_for(runs) {
        return Err("the window has not as many directories as its runs need");
    }
    let held = memory::with(|m| {
        m.frame_in_use(built.pdpt) && directories.iter().all(|&d| m.frame_in_use(d))
    });
    if !held {
        return Err("a window table's frame was freed");
    }
    let want = window::pdpt(directories).map_err(|_| "the window's PDPT is broken")?;
    if read(built.pdpt) != want {
        return Err("the window's PDPT is not what was built");
    }
    for (d, &directory) in directories.iter().enumerate() {
        let want = window::directory(d as u64, runs, &run).map_err(|_| "a run is misplaced")?;
        if read(directory) != want {
            return Err("a window directory is not what was built");
        }
    }
    Ok(())
}

fn write(table: u64, entries: &[u64; 512]) {
    // SAFETY: `table` is a frame allocated for the window alone and not yet linked
    // anywhere, reached through the direct map; 512 entries fill exactly one page.
    unsafe {
        core::ptr::copy_nonoverlapping(entries.as_ptr(), (memory::hhdm() + table) as *mut u64, 512)
    };
}

fn read(table: u64) -> [u64; 512] {
    let mut entries: [u64; 512] = [0; 512];
    // SAFETY: as above; the window's tables are never freed, and nothing writes them
    // after `build` but the CPU's Accessed and Dirty bits, which are masked here.
    unsafe {
        core::ptr::copy_nonoverlapping(
            (memory::hhdm() + table) as *const u64,
            entries.as_mut_ptr(),
            512,
        )
    };
    entries.map(|entry| entry & !(window::ACCESSED | window::DIRTY))
}
