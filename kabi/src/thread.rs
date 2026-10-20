//! Worker threads: how many a process may run beside its main thread, and where
//! their stacks go. A process holding [`crate::cap::THREADS`] starts workers with
//! [`crate::call::THREAD_SPAWN`]; each is pinned to a CPU and runs in the process's
//! address space until it calls [`crate::call::THREAD_EXIT`] or the process ends.
//!
//! Worker `i` has a window of [`WINDOW`] bytes ending at [`stack_top`]`(i)`. Its stack
//! takes the top of the window, at most [`MAX_STACK_PAGES`] pages, and the rest is
//! never mapped, so a stack that overflows faults instead of running into the next
//! worker's. A later worker with the same index reuses the pages already mapped.

use crate::layout::{MODEL_BASE, MODEL_MAX, STACK_TOP, USER_END};

/// Workers a process may run at once, besides its main thread: one for each CPU
/// the kernel can bring up.
pub const MAX_WORKERS: usize = 8;
/// Largest stack a worker may ask for, in pages: 1 MiB.
pub const MAX_STACK_PAGES: u64 = 256;
/// Bytes of address space each worker's stack window spans.
pub const WINDOW: u64 = 2 << 20;
/// Pages a pool of [`MAX_WORKERS`] workers with 16-page stacks takes in its
/// process's quota, page tables included (a PDPT, a directory and one table per
/// window): what a service's plan line adds for `user::pool`.
pub const POOL_PAGES: u64 = MAX_WORKERS as u64 * (16 + 1) + 2;
/// Top of worker 0's window; the others lie below it, one window each.
pub const STACKS_TOP: u64 = 0x0000_7F00_0000_0000;

/// The address worker `index`'s stack grows down from.
pub const fn stack_top(index: usize) -> u64 {
    STACKS_TOP - index as u64 * WINDOW
}

/// The lowest address of worker `index`'s stack when it has `pages` pages.
pub const fn stack_bottom(index: usize, pages: u64) -> u64 {
    stack_top(index) - pages * 4096
}

// Every window lies in user space, above the model's slot and below the main
// stack (and the unmapped page under it), and the largest stack leaves at least
// 1 MiB of its window unmapped.
const _: () = {
    let lowest = STACKS_TOP - MAX_WORKERS as u64 * WINDOW;
    assert!(lowest >= MODEL_BASE + MODEL_MAX);
    assert!(STACKS_TOP < STACK_TOP - crate::layout::MAX_STACK_PAGES * 4096 - 4096);
    assert!(STACKS_TOP < USER_END);
    assert!(WINDOW - MAX_STACK_PAGES * 4096 >= 1 << 20);
    assert!(STACKS_TOP.is_multiple_of(WINDOW));
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_are_apart_and_each_keeps_a_guard() {
        for index in 0..MAX_WORKERS {
            let top = stack_top(index);
            let window_bottom = top - WINDOW;
            assert_eq!(top % 16, 0);
            assert!(stack_bottom(index, MAX_STACK_PAGES) - window_bottom >= 1 << 20);
            if index + 1 < MAX_WORKERS {
                assert_eq!(stack_top(index + 1), window_bottom);
            }
        }
        assert_eq!(stack_top(0), STACKS_TOP);
        assert_eq!(stack_bottom(3, 16), STACKS_TOP - 3 * WINDOW - 16 * 4096);
    }
}
