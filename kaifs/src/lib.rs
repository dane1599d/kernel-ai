//! kaifs, Kernel AI's copy-on-write filesystem (ADR 0004), format 1.
//!
//! A change never overwrites data in place: it goes to free blocks, and one small
//! record written last, the superblock, makes it live. A crash leaves the old
//! state or the new one. This crate is `no_std` + `alloc`, so the ring-3 `fs`
//! service, `cargo xtask fs` and the host tests run the same code over a
//! [`BlockDev`].
//!
//! Built so far (P2 task 4, written fresh from spike S04-1's design):
//! - [`format`]: the 64-byte [`BlockPtr`], the 4 KiB [`Superblock`], its two rings
//!   of eight slots, and the mount rules;
//! - [`dev`]: the [`BlockDev`] trait, an in-memory disk, a recording disk, and the
//!   [`Image`] a crash leaves;
//! - [`buddy`] and [`allocator`]: the buddy allocator, the N+2 wait for freed
//!   blocks, and the allocator's chain on disk;
//! - [`crash`] (with `std`): the crash enumerator every later layer is gated by.

#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod allocator;
pub mod buddy;
#[cfg(feature = "std")]
pub mod crash;
pub mod csum;
pub mod dev;
pub mod format;

pub use csum::CsumKind;
pub use dev::{BlockDev, DevInfo, Image, IoError, MemDisk, Op, RecordingDisk};
pub use format::{BlockPtr, Superblock};

/// Bytes in a block, and in a tree node.
pub const BLOCK: usize = 4096;
/// One block's bytes.
pub type Block = [u8; BLOCK];
/// Superblock slots in each ring.
pub const SLOTS: u64 = 8;

/// Why an operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The device failed a read, a write or a flush.
    Io,
    /// A checksum or a structure did not hold: what it was.
    Corrupt(&'static str),
    /// No free run is large enough.
    NoSpace,
    /// No superblock slot holds a commit that verifies.
    NoValidSuperblock,
    /// The disk needs something this build does not have: what it is.
    Unsupported(&'static str),
    /// The filesystem is mounted read-only.
    ReadOnly,
}

impl From<IoError> for Error {
    fn from(_: IoError) -> Self {
        Error::Io
    }
}
