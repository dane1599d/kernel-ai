//! Memory-management logic that does not touch hardware, so it can be unit-tested on the host.
//!
//! - [`FrameBitmap`]: physical frame allocator, one bit per 4 KiB frame.
//! - [`ReclaimPolicy`] and [`Clock`]: the swappable page-reclaim policy and its default.
//! - [`dma`]: which buffers a device can reach unbounced, and sector-sized disk reads.
//! - [`window`]: the model window's page tables, 2 MiB read-only pages in one slot.

#![no_std]

extern crate alloc;

pub mod dma;
mod frames;
mod reclaim;
pub mod window;

pub use frames::{FrameBitmap, FrameError};
pub use reclaim::{Clock, ReclaimPolicy};

pub const PAGE_SIZE: u64 = 4096;
