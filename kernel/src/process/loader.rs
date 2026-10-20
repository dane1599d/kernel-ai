//! Loading a program into a fresh address space, and growing its heap.

use super::{SLOT_OF_TID, SPACES, State, exit_current, killed, programs, request_kill, with_table};
use crate::entry;
use crate::memory::user::{Access, MIN_USER_ADDRESS};
use crate::{log, memory, threads};
use core::sync::atomic::Ordering;
use kabi::layout::{HEAP_BASE, MAX_STACK_PAGES, STACK_PAGES, STACK_TOP};
use kabi::{ExitReason, Limits};
use mm::PAGE_SIZE;

/// Where a loaded program starts.
#[derive(Clone, Copy)]
struct UserEntry {
    rip: u64,
    rsp: u64,
    cr3: u64,
}

/// The process thread: load the program, then enter ring 3 and never come back here.
pub(super) fn process_main(slot: usize) {
    let me = threads::current_tid();
    SLOT_OF_TID[me].store(slot + 1, Ordering::Release);
    let (name, program, limits) = with_table(|table| {
        table[slot].tid = Some(me);
        (table[slot].name, table[slot].program, table[slot].limits)
    });
    let entry = match load(slot, program.as_str(), &limits) {
        Ok(entry) => entry,
        Err(error) => {
            log!("process {}: cannot load: {error}", name.as_str());
            if error == QUOTA {
                request_kill(slot, ExitReason::OutOfMemory, limits.max_pages);
            } else {
                request_kill(slot, ExitReason::LoadFailed, 0);
            }
            exit_current();
        }
    };
    // A kill that came before this thread was known could not wake it.
    if killed(slot) {
        exit_current();
    }
    with_table(|table| table[slot].state = State::Running);
    threads::enter_address_space(entry.cr3);
    // Clean vector registers; interrupts stay off until the iretq.
    crate::fpu::enter_ring3();
    // SAFETY: the entry point and stack were mapped by `load`, CR3 is loaded, and
    // nothing on this stack needs dropping.
    unsafe { entry::enter_user(entry.rip, entry.rsp) }
}

const QUOTA: &str = "page quota exceeded";

/// Maps a fresh page into the process's address space, within its page quota.
fn map_page(slot: usize, addr: u64, access: Access, max_pages: u64) -> Result<u64, &'static str> {
    let mut guard = SPACES[slot].lock();
    let space = guard.as_mut().ok_or("no address space")?;
    if space.frames() >= max_pages {
        return Err(QUOTA);
    }
    let phys = memory::with(|m| m.map_user_page(space, addr, access))?;
    if space.frames() > max_pages {
        return Err(QUOTA);
    }
    Ok(phys)
}

/// Builds the address space: program segments and the stack.
fn load(slot: usize, name: &str, limits: &Limits) -> Result<UserEntry, &'static str> {
    let program = programs::find(name).ok_or("no such program")?;
    let image = uelf::parse(program.elf, MIN_USER_ADDRESS..HEAP_BASE).map_err(elf_error)?;
    let space = memory::with(|m| m.new_user_space())?;
    let cr3 = space.cr3();
    *SPACES[slot].lock() = Some(space);
    let hhdm = memory::hhdm();
    for segment in image.segments() {
        let access = Access {
            write: segment.write,
            execute: segment.execute,
        };
        for page in segment.pages().step_by(PAGE_SIZE as usize) {
            let phys = map_page(slot, page, access, limits.max_pages)?;
            // File bytes that land in this page, if any.
            let file_start = segment.vaddr;
            let file_end = segment.vaddr + segment.file.len() as u64;
            let from = page.max(file_start);
            let to = (page + PAGE_SIZE).min(file_end);
            if from < to {
                let source = segment.file.start + (from - file_start) as usize;
                let bytes = &program.elf[source..source + (to - from) as usize];
                // SAFETY: the frame was just mapped for this process alone and is
                // reached through the HHDM; the copy stays inside the page.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        (hhdm + phys + (from - page)) as *mut u8,
                        bytes.len(),
                    )
                };
            }
        }
    }
    let stack = Access {
        write: true,
        execute: false,
    };
    let stack_pages = match limits.stack_pages {
        0 => STACK_PAGES,
        pages => pages.min(MAX_STACK_PAGES),
    };
    for page in 1..=stack_pages {
        map_page(slot, STACK_TOP - page * PAGE_SIZE, stack, limits.max_pages)?;
    }
    Ok(UserEntry {
        rip: image.entry,
        // As if `_start` had been called: RSP is 8 below a 16-byte boundary.
        rsp: STACK_TOP - 8,
        cr3,
    })
}

fn elf_error(error: uelf::Error) -> &'static str {
    match error {
        uelf::Error::SegmentOutOfRange => "a segment lies outside user memory",
        uelf::Error::WritableAndExecutable => "a segment is writable and executable",
        uelf::Error::Overlap => "segments overlap",
        uelf::Error::BadEntry => "the entry point is not in executable code",
        uelf::Error::Unsupported => "dynamic linking, an interpreter or TLS",
        _ => "not a valid x86-64 executable",
    }
}

/// Maps the kernel's `frames` (frame numbers) read-only from `base` up in the
/// calling process, without making them its own (see `Memory::map_user_frame`).
/// `base` starts a PML4 slot that holds nothing else, and `tables` is how many
/// page tables the whole mapping takes there. The call fails with NoMemory, and
/// kills nothing, if the tables still missing would pass the page quota. Pages
/// already mapped stay as they are, so the call can be repeated.
pub fn map_shared(slot: usize, base: u64, frames: &[u32], tables: u64) -> Result<(), kabi::Error> {
    let limits = with_table(|table| table[slot].limits);
    let mut guard = SPACES[slot].lock();
    let space = guard.as_mut().ok_or(kabi::Error::Fault)?;
    // Tables a first call, or one cut short, already made are counted already.
    let missing = tables.saturating_sub(memory::with(|m| m.tables_in_slot(space, base)));
    if space.frames().saturating_add(missing) > limits.max_pages {
        return Err(kabi::Error::NoMemory);
    }
    // One page table's worth under each MEMORY lock, so other CPUs are not held
    // off for the whole model.
    for (index, batch) in frames.chunks(512).enumerate() {
        let first = base + (index * 512) as u64 * PAGE_SIZE;
        memory::with(|m| {
            batch.iter().enumerate().try_for_each(|(i, &frame)| {
                let addr = first + i as u64 * PAGE_SIZE;
                m.map_user_frame(space, addr, u64::from(frame) * PAGE_SIZE)
                    .map(|_| ())
            })
        })
        .map_err(|error| {
            log!("process: mapping shared frames failed: {error}");
            kabi::Error::NoMemory
        })?;
    }
    Ok(())
}

/// Grows the calling process's heap by `bytes` (whole pages). Exceeding the quota
/// kills the process; running the machine out of frames only fails the call.
pub fn grow_heap(slot: usize, bytes: u64) -> Result<u64, kabi::Error> {
    let (end, limits) = with_table(|table| (table[slot].heap_end, table[slot].limits));
    if bytes == 0 {
        return Ok(end);
    }
    let pages = bytes.div_ceil(PAGE_SIZE);
    let new_end = pages
        .checked_mul(PAGE_SIZE)
        .and_then(|grow| end.checked_add(grow))
        .filter(|&new_end| new_end <= HEAP_BASE + kabi::layout::HEAP_MAX)
        .ok_or(kabi::Error::NoMemory)?;
    let heap = Access {
        write: true,
        execute: false,
    };
    let mut mapped_to = end;
    let result: Result<(), &str> =
        (end..new_end)
            .step_by(PAGE_SIZE as usize)
            .try_for_each(|page| {
                map_page(slot, page, heap, limits.max_pages)?;
                mapped_to = page + PAGE_SIZE;
                Ok(())
            });
    with_table(|table| table[slot].heap_end = mapped_to);
    match result {
        Ok(()) => Ok(new_end),
        Err(error) if error == QUOTA => {
            request_kill(slot, ExitReason::OutOfMemory, limits.max_pages);
            Err(kabi::Error::NoMemory)
        }
        Err(error) => {
            log!("process: heap growth failed: {error}");
            Err(kabi::Error::NoMemory)
        }
    }
}
