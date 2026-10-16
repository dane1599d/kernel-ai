//! Memory-management logic that does not touch hardware, so it can be unit-tested on the host.
//!
//! - [`FrameBitmap`]: physical frame allocator, one bit per 4 KiB frame.
//! - [`ReclaimPolicy`] and [`Clock`]: the swappable page-reclaim policy and its default.

#![no_std]

extern crate alloc;

mod frames;
mod reclaim;

pub use frames::{FrameBitmap, FrameError};
pub use reclaim::{Clock, ReclaimPolicy};

pub const PAGE_SIZE: u64 = 4096;
