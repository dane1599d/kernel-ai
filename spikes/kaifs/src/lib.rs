//! Spike S04-1 (ADR 0004): kaifs's crash core, host-only.
//!
//! Superblock rings, `BlockPtr`, one copy-on-write B+tree of u64 keys per tree
//! (values of at most 1 KiB, or a pointer to a data record of up to 128 KiB), a
//! buddy allocator, commit with two flushes, snapshots, the N+2 reuse rule,
//! mark-and-sweep and `check()`. The crash harness (`crash`) replays a
//! `RecordingDisk`'s log at every crash point the ADR names and checks the oracle.
//! Never merged: kaifs proper lands in P2 from what this spike settles.

#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod alloc_buddy;
pub mod check;
#[cfg(feature = "std")]
pub mod crash;
pub mod csum;
pub mod disk;
pub mod format;
pub mod fs;
pub mod tree;

pub use csum::CsumKind;
pub use disk::{BlockDev, Image, MemDisk, Op, RecordingDisk};
pub use format::BlockPtr;
pub use fs::{Error, Fs, Value};

/// Bytes in a block, and in a tree node.
pub const BLOCK: usize = 4096;
/// One block's bytes.
pub type Block = [u8; BLOCK];
/// Superblock slots in each ring.
pub const SLOTS: u64 = 8;
/// The largest inline tree value.
pub const MAX_VALUE: usize = 1024;
/// The largest data record: 128 KiB.
pub const MAX_RECORD: usize = 128 << 10;
