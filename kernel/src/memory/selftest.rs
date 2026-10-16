//! M2 self-tests: kernel half, map/unmap/protect, heap stress, CLOCK on real accessed
//! bits, plus triggers for the write-protect and stack-overflow fault cases.

use super::stack::GuardedStack;
use super::{SCRATCH_START, heap, with};
use crate::faults;
use crate::log;
use alloc::alloc::{alloc, dealloc};
use alloc::vec::Vec;
use core::alloc::Layout;
use mm::{Clock, PAGE_SIZE, ReclaimPolicy};
use x86_64::structures::paging::PageTableFlags;

const RW: PageTableFlags = PageTableFlags::PRESENT
    .union(PageTableFlags::WRITABLE)
    .union(PageTableFlags::NO_EXECUTE);
const READ_ONLY: PageTableFlags = PageTableFlags::PRESENT.union(PageTableFlags::NO_EXECUTE);
const HEAP_OPS: u32 = 1_000_000;
const HEAP_SLOTS: usize = 256;
const CLOCK_PAGES: u64 = 8;

/// Runs every M2 boot check.
pub fn run() -> Result<(), &'static str> {
    if !with(|memory| memory.kernel_half_complete()) {
        return Err("kernel half has empty PML4 slots");
    }
    map_unmap_protect()?;
    heap_stress()?;
    clock_on_real_accessed_bits()?;
    log!("self-test: memory checks passed");
    Ok(())
}

fn map_unmap_protect() -> Result<(), &'static str> {
    let addr = SCRATCH_START;
    // Warm up so the intermediate page tables exist before counting frames.
    with(|m| m.map(addr, RW).and_then(|()| m.unmap(addr)))?;
    let free_before = with(|m| m.free_frames());

    with(|m| m.map(addr, RW))?;
    let ptr = addr as *mut u64;
    // SAFETY: the page at addr was just mapped writable.
    let read_back = unsafe {
        ptr.write_volatile(0x4B41_495F_4D32_4F4B);
        ptr.read_volatile()
    };
    if read_back != 0x4B41_495F_4D32_4F4B {
        return Err("mapped page did not keep its data");
    }
    with(|m| m.protect(addr, READ_ONLY))?;
    let flags = with(|m| m.translate(addr)).map(|(_, f)| f);
    if flags.is_none_or(|f| f.contains(PageTableFlags::WRITABLE)) {
        return Err("protect did not clear the writable bit");
    }
    with(|m| m.unmap(addr))?;
    if with(|m| m.translate(addr)).is_some() {
        return Err("page still translates after unmap");
    }
    if with(|m| m.free_frames()) != free_before {
        return Err("map and unmap leaked a frame");
    }
    log!("self-test: map, write, protect, unmap ok; no frame leaked");
    Ok(())
}

/// One million random alloc/free operations, then a leak check against the counters.
fn heap_stress() -> Result<(), &'static str> {
    let baseline = heap::live();
    let mut slots: [Option<(*mut u8, Layout)>; HEAP_SLOTS] = [None; HEAP_SLOTS];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..HEAP_OPS {
        let roll = next();
        let slot = &mut slots[(roll % HEAP_SLOTS as u64) as usize];
        match slot.take() {
            Some((ptr, layout)) => {
                // SAFETY: ptr came from alloc with this layout and holds its tag byte.
                unsafe {
                    if ptr.read() != (layout.size() as u8) {
                        return Err("heap allocation was overwritten");
                    }
                    dealloc(ptr, layout);
                }
            }
            None => {
                let size = 1 + (roll >> 8) as usize % 2048;
                let align = 1 << ((roll >> 20) % 7);
                let layout = Layout::from_size_align(size, align).map_err(|_| "bad layout")?;
                // SAFETY: layout has non-zero size.
                let ptr = unsafe { alloc(layout) };
                if ptr.is_null() {
                    return Err("heap ran out of memory during stress test");
                }
                // SAFETY: ptr is a fresh allocation of at least one byte.
                unsafe { ptr.write(size as u8) };
                *slot = Some((ptr, layout));
            }
        }
    }
    for (ptr, layout) in slots.iter().flatten() {
        // SAFETY: each live slot holds a pointer from alloc with its layout.
        unsafe { dealloc(*ptr, *layout) };
    }
    if heap::live() != baseline {
        return Err("heap stress test leaked allocations");
    }
    log!("self-test: heap stress {HEAP_OPS} ops, no leak");
    Ok(())
}

/// CLOCK must reclaim exactly the pages the CPU did not touch.
fn clock_on_real_accessed_bits() -> Result<(), &'static str> {
    let base = SCRATCH_START + 0x10_0000;
    let pages: Vec<u64> = (0..CLOCK_PAGES).map(|i| base + i * PAGE_SIZE).collect();
    let free_before = with(|m| m.free_frames());
    let mut clock = Clock::new();
    for &page in &pages {
        with(|m| m.map(page, RW))?;
        with(|m| m.take_accessed(page)).ok_or("scratch page not mapped")?;
        clock.track(page);
    }
    for &page in pages.iter().step_by(2) {
        // SAFETY: page is mapped writable; the write sets its accessed bit.
        unsafe { (page as *mut u64).write_volatile(1) };
    }
    let mut victims = Vec::new();
    clock.select(
        (CLOCK_PAGES / 2) as usize,
        &mut |page| with(|m| m.take_accessed(page)).unwrap_or(false),
        &mut victims,
    );
    let untouched: Vec<u64> = pages.iter().copied().skip(1).step_by(2).collect();
    pages.iter().try_for_each(|&page| with(|m| m.unmap(page)))?;
    if victims != untouched {
        return Err("CLOCK did not pick exactly the untouched pages");
    }
    if with(|m| m.free_frames()) < free_before {
        return Err("CLOCK test leaked frames");
    }
    log!(
        "self-test: {} reclaimed {} untouched pages via real accessed bits",
        clock.name(),
        victims.len()
    );
    Ok(())
}

/// Maps a page, makes it read-only, then writes to it.
pub fn trigger_write_protect() -> Result<(), &'static str> {
    let addr = SCRATCH_START + 0x20_0000;
    with(|m| m.map(addr, RW).and_then(|()| m.protect(addr, READ_ONLY)))?;
    faults::expect_write_fault(addr);
    // SAFETY: deliberately faults; the page-fault handler never returns here.
    unsafe { (addr as *mut u64).write_volatile(1) };
    Err("write to a read-only page did not fault")
}

/// Switches to a small guarded stack and recurses until it hits the guard page.
pub fn trigger_stack_overflow() -> Result<(), &'static str> {
    let stack = GuardedStack::new(4)?;
    faults::expect_stack_overflow(stack.guard);
    // SAFETY: stack.top is the top of a freshly mapped stack; recurse never returns.
    unsafe {
        core::arch::asm!(
            "mov rsp, {top}",
            "xor rbp, rbp",
            "call {recurse}",
            top = in(reg) stack.top,
            recurse = sym recurse,
            options(noreturn)
        )
    }
}

// Recursing until the guard page is hit is the point of this test.
#[allow(unconditional_recursion)]
extern "C" fn recurse() -> ! {
    let mut frame = [0u8; 512];
    // SAFETY: writes to this function's own stack frame keep it from being optimized out.
    unsafe { core::ptr::write_volatile(&mut frame[0], 1) };
    recurse()
}
