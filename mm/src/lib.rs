//! Memory-management logic that does not touch hardware, so it can be unit-tested on the host.
//!
//! - [`FrameBitmap`]: physical frame allocator, one bit per 4 KiB frame.
//! - [`ReclaimPolicy`] and [`Clock`]: the swappable page-reclaim policy and its default.
//! - [`bounce`]: the virtio bounce pool's fixed-size slots (P6 G66).
//! - [`dma`]: which buffers a device can reach unbounced, and sector-sized disk reads.
//! - [`window`]: the model window's page tables, 2 MiB read-only pages in one slot.

#![no_std]

extern crate alloc;

pub mod bounce;
pub mod dma;
mod frames;
mod reclaim;
pub mod reserve;
pub mod window;

pub use frames::{FrameBitmap, FrameError};
pub use reclaim::{Clock, ReclaimPolicy};

pub const PAGE_SIZE: u64 = 4096;

/// The pages holding `len` bytes (at least one) of physical memory from `phys`:
/// the first page and how many; `None` past 2^64 or past `limit`, one past the
/// highest address the CPU can map (firmware tables may name any address).
pub fn physical_pages(phys: u64, len: u64, limit: u64) -> Option<(u64, u64)> {
    let end = phys.checked_add(len.max(1))?;
    if end > limit {
        return None;
    }
    let first = phys / PAGE_SIZE * PAGE_SIZE;
    Some((first, end.div_ceil(PAGE_SIZE) - first / PAGE_SIZE))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: u64 = 1 << 40;

    fn at(phys: u64, len: u64) -> Option<(u64, u64)> {
        physical_pages(phys, len, LIMIT)
    }

    #[test]
    fn a_range_is_rounded_out_to_whole_pages() {
        assert_eq!(at(0xFED0_0000, 0x400), Some((0xFED0_0000, 1)));
        assert_eq!(at(0x1FFC, 8), Some((0x1000, 2)));
        assert_eq!(at(0x5000, 0), Some((0x5000, 1)));
        assert_eq!(at(LIMIT - 4, 4), Some((LIMIT - PAGE_SIZE, 1)));
    }

    #[test]
    fn addresses_past_the_cpu_or_past_2_64_are_refused() {
        let wraps = [(u64::MAX, 4), (u64::MAX - 3, 4), (u64::MAX, 0)];
        let past = [(0, u64::MAX), (1 << 52, 4), (LIMIT, 1), (LIMIT - 4, 5)];
        for (phys, len) in wraps.into_iter().chain(past) {
            assert_eq!(at(phys, len), None, "{phys:#x} + {len:#x}");
        }
    }
}
