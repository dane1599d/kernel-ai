//! The model window's page tables (L6), as pure functions so the host can test
//! them: one PML4 slot (512 GiB) whose PDPT points at one page directory per GiB,
//! whose entries map 2 MiB pages (PS=1) of physically contiguous, 2 MiB-aligned runs.
//!
//! Every entry is computed whole, all 512 of each table, so a table never holds
//! anything but a run or zero. Every level is PRESENT and USER and never WRITABLE,
//! and every level is no-execute, so ring 3 can only read the model. The kernel
//! builds the tables once and links the PDPT into each process that maps the model.

use crate::PAGE_SIZE;

/// Frames in one run, and its size: one 2 MiB page.
pub const RUN_FRAMES: u64 = 512;
pub const RUN_BYTES: u64 = RUN_FRAMES * PAGE_SIZE;
/// Bytes one page directory maps.
pub const DIRECTORY_BYTES: u64 = 512 * RUN_BYTES;
/// Most runs one window holds: a whole PML4 slot.
pub const MAX_RUNS: u64 = 512 * 512;

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const USER: u64 = 1 << 2;
pub const ACCESSED: u64 = 1 << 5;
pub const DIRTY: u64 = 1 << 6;
pub const HUGE: u64 = 1 << 7;
pub const NO_EXECUTE: u64 = 1 << 63;
/// Physical address bits an entry holds (bits 12 to 51).
pub const ADDRESS: u64 = 0x000F_FFFF_FFFF_F000;

/// Flags of a PML4 or PDPT entry pointing at the next table.
pub const TABLE_FLAGS: u64 = PRESENT | USER | NO_EXECUTE;
/// Flags of a directory entry mapping one run.
pub const RUN_FLAGS: u64 = PRESENT | USER | HUGE | NO_EXECUTE;

/// Runs a window of `len` bytes takes.
pub const fn runs_for(len: u64) -> u64 {
    len.div_ceil(RUN_BYTES)
}

/// Page directories a window of `runs` runs takes.
pub const fn directories_for(runs: u64) -> u64 {
    runs.div_ceil(512)
}

/// Frames the window's tables take: the directories and the PDPT.
pub const fn table_frames(runs: u64) -> u64 {
    if runs == 0 {
        0
    } else {
        directories_for(runs) + 1
    }
}

/// Why a window cannot be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowError {
    /// No runs, or more than a PML4 slot holds.
    Size,
    /// A run's address is not 2 MiB-aligned or does not fit an entry.
    Run(u64),
    /// A table's address does not fit an entry or is not page-aligned.
    Table(u64),
}

fn entry(phys: u64, flags: u64) -> u64 {
    phys | flags
}

fn valid_run(phys: u64) -> bool {
    phys.is_multiple_of(RUN_BYTES) && phys & !ADDRESS == 0
}

fn valid_table(phys: u64) -> bool {
    phys.is_multiple_of(PAGE_SIZE) && phys & !ADDRESS == 0
}

/// The 512 entries of directory `directory` of a window of `runs` runs, where run
/// `k` of the window is at physical address `run(k)`.
pub fn directory(
    directory: u64,
    runs: u64,
    run: impl Fn(u64) -> u64,
) -> Result<[u64; 512], WindowError> {
    if runs == 0 || runs > MAX_RUNS || directory >= directories_for(runs) {
        return Err(WindowError::Size);
    }
    let mut table = [0; 512];
    for (index, slot) in table.iter_mut().enumerate() {
        let k = directory * 512 + index as u64;
        if k >= runs {
            break;
        }
        let phys = run(k);
        if !valid_run(phys) {
            return Err(WindowError::Run(phys));
        }
        *slot = entry(phys, RUN_FLAGS);
    }
    Ok(table)
}

/// The 512 entries of the PDPT, pointing at the directories at `directories`.
pub fn pdpt(directories: &[u64]) -> Result<[u64; 512], WindowError> {
    if directories.is_empty() || directories.len() > 512 {
        return Err(WindowError::Size);
    }
    let mut table = [0; 512];
    for (slot, &phys) in table.iter_mut().zip(directories) {
        if !valid_table(phys) {
            return Err(WindowError::Table(phys));
        }
        *slot = entry(phys, TABLE_FLAGS);
    }
    Ok(table)
}

/// The PML4 entry that links a window whose PDPT is at `pdpt`.
pub fn link(pdpt: u64) -> Result<u64, WindowError> {
    if valid_table(pdpt) {
        Ok(entry(pdpt, TABLE_FLAGS))
    } else {
        Err(WindowError::Table(pdpt))
    }
}

/// Whether a PML4 entry links the window at `pdpt`, whatever the Accessed bit
/// the CPU set says.
pub fn links(pml4_entry: u64, pdpt: u64) -> bool {
    pml4_entry & PRESENT != 0 && pml4_entry & ADDRESS == pdpt
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    const GIB: u64 = 1 << 30;

    #[test]
    fn every_entry_is_a_read_only_no_execute_run_or_zero() {
        // Runs above 4 GiB and near the top of what an entry holds.
        let base = [4 * GIB, 64 * GIB, (1 << 52) - RUN_BYTES];
        for start in base {
            let runs = 700;
            let run = |k: u64| start - (k % 3) * RUN_BYTES;
            for d in 0..directories_for(runs) {
                let table = directory(d, runs, run).unwrap();
                for (i, &e) in table.iter().enumerate() {
                    let k = d * 512 + i as u64;
                    if k < runs {
                        assert_eq!(e & ADDRESS, run(k), "run {k}");
                        assert_eq!(e & !ADDRESS, RUN_FLAGS);
                        assert_eq!(e & WRITABLE, 0);
                    } else {
                        assert_eq!(e, 0);
                    }
                }
            }
        }
    }

    #[test]
    fn windows_of_one_five_and_512_gib() {
        for (bytes, directories) in [(GIB, 1), (5 * GIB + RUN_BYTES, 6), (512 * GIB, 512)] {
            let runs = runs_for(bytes);
            assert_eq!(directories_for(runs), directories);
            assert_eq!(table_frames(runs), directories + 1);
            let last = directory(directories - 1, runs, |k| k * RUN_BYTES).unwrap();
            let used = runs - (directories - 1) * 512;
            assert!(last[..used as usize].iter().all(|&e| e & PRESENT != 0));
            assert!(last[used as usize..].iter().all(|&e| e == 0));
        }
        assert_eq!(table_frames(0), 0);
        assert_eq!(runs_for(639_446_688), 305);
        assert_eq!(directory(0, MAX_RUNS + 1, |_| 0), Err(WindowError::Size));
        assert_eq!(directory(1, 512, |_| 0), Err(WindowError::Size));
    }

    #[test]
    fn misaligned_or_oversized_addresses_are_refused() {
        assert_eq!(
            directory(0, 1, |_| RUN_BYTES + PAGE_SIZE),
            Err(WindowError::Run(RUN_BYTES + PAGE_SIZE))
        );
        assert_eq!(directory(0, 1, |_| 1 << 52), Err(WindowError::Run(1 << 52)));
        assert_eq!(pdpt(&[0x1001]), Err(WindowError::Table(0x1001)));
        assert_eq!(pdpt(&[]), Err(WindowError::Size));
        let many: Vec<u64> = (0..513).map(|i| i * PAGE_SIZE).collect();
        assert_eq!(pdpt(&many), Err(WindowError::Size));
    }

    #[test]
    fn the_pdpt_and_link_point_at_tables_without_write() {
        let table = pdpt(&[0x5000, 5 * GIB + 0x2000]).unwrap();
        assert_eq!(table[0], 0x5000 | TABLE_FLAGS);
        assert_eq!(table[1], (5 * GIB + 0x2000) | TABLE_FLAGS);
        assert!(table[2..].iter().all(|&e| e == 0));
        let linked = link(0x7000).unwrap();
        assert_eq!(linked & WRITABLE, 0);
        assert!(
            links(linked | (1 << 5), 0x7000),
            "the Accessed bit does not matter"
        );
        assert!(!links(linked, 0x8000));
        assert!(!links(0, 0));
    }
}
