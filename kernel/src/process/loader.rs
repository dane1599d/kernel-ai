//! Loading a program into a fresh address space, and growing its heap.

use super::{SLOT_OF_TID, SPACES, State, exit_current, killed, programs, request_kill, with_table};
use crate::entry;
use crate::memory::user::{Access, MIN_USER_ADDRESS, UserSpace};
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
    // Before the slot is published, from when on this thread is charged.
    super::burst::start_thread(me);
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
    with_table(|table| {
        table[slot].cr3 = entry.cr3;
        table[slot].state = State::Running;
    });
    threads::enter_address_space(entry.cr3);
    // Clean vector registers; interrupts stay off until the iretq.
    crate::fpu::enter_ring3();
    // SAFETY: the entry point and stack were mapped by `load`, CR3 is loaded, and
    // nothing on this stack needs dropping.
    unsafe { entry::enter_user(entry.rip, entry.rsp, 0) }
}

const QUOTA: &str = "page quota exceeded";

/// Maps a fresh page into the process's address space, within its page quota.
fn map_page(slot: usize, addr: u64, access: Access, max_pages: u64) -> Result<u64, &'static str> {
    let mut guard = SPACES[slot].lock();
    let space = guard.as_mut().ok_or("no address space")?;
    map_page_in(space, addr, access, max_pages)
}

/// [`map_page`] with the space already locked.
fn map_page_in(
    space: &mut UserSpace,
    addr: u64,
    access: Access,
    max_pages: u64,
) -> Result<u64, &'static str> {
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

/// Links the model's window, whose PDPT is at `pdpt`, at `base` in the calling
/// process: one PML4 entry, no frame taken from its quota. Repeating it changes
/// nothing.
pub fn link_window(slot: usize, base: u64, pdpt: u64) -> Result<(), kabi::Error> {
    let mut guard = SPACES[slot].lock();
    let space = guard.as_mut().ok_or(kabi::Error::Fault)?;
    memory::with(|m| m.link_window(space, base, pdpt)).map_err(|error| {
        log!("process: linking the model window failed: {error}");
        kabi::Error::Invalid
    })
}

/// Grows the calling process's heap by `bytes` (whole pages). Exceeding the quota
/// kills the process; running the machine out of frames only fails the call. The
/// space stays locked from reading the heap's end to writing it back, so two
/// threads growing the heap at once grow it one after the other.
pub fn grow_heap(slot: usize, bytes: u64) -> Result<u64, kabi::Error> {
    let mut guard = SPACES[slot].lock();
    let space = guard.as_mut().ok_or(kabi::Error::Fault)?;
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
                map_page_in(space, page, heap, limits.max_pages)?;
                mapped_to = page + PAGE_SIZE;
                Ok(())
            });
    with_table(|table| table[slot].heap_end = mapped_to);
    drop(guard);
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
