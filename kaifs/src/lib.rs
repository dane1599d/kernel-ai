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
//! - [`tree`]: the copy-on-write B+tree of `(oid, kind, off)` keys;
//! - [`io`]: what lies below the trees: records, values and the reuse rule
//!   decided at replacement;
//! - [`fs`]: the engine: volumes, each a live tree and its snapshots, the volume
//!   table, commit and mount;
//! - [`check`]: everything a mount reaches verifies and is accounted for, with
//!   the memo the crash gates share;
//! - [`crash`] (with `std`): the crash enumerator every later layer is gated by.
//!
//! P2 task 5 adds the namespace on top:
//! - [`items`]: files, folders, names, data and extended attributes as tree
//!   items, and their readers;
//! - [`ns`]: the operations on them;
//! - [`ns_check`]: the namespace's rules, checked through the same memo, and the
//!   digest the crash gates compare;
//! - [`repair`] and [`repair_fs`]: `check --repair`'s rules, on a volume's
//!   namespace and on the whole filesystem.
//!
//! P2 task 6 adds volumes and undo:
//! - [`vol`]: the volume table's entries: volumes, snapshots of four kinds, and
//!   UNDO records;
//! - [`txn`]: actions, each one commit with its undo snapshots and UNDO records,
//!   and abort;
//! - [`undo`]: `restore(txn)`, which never overwrites a later change; `changes`,
//!   which reads only what changed; and `history`;
//! - [`sealed`]: sealed files, the model store's blobs in 2 MiB records;
//! - [`trash`] and [`cbor`]: the trash, each object beside its `info.cbor`;
//! - [`gc`]: the GC in slices, and retention.
//!
//! For P2 task 8's `fs` server: [`cache`], a write-through CLOCK block cache over
//! any device.

#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod allocator;
pub mod buddy;
pub mod cache;
pub mod cbor;
pub mod check;
#[cfg(feature = "std")]
pub mod crash;
pub mod csum;
pub mod dev;
pub mod format;
pub mod fs;
pub mod gc;
pub mod io;
pub mod items;
pub mod ns;
pub mod ns_check;
pub mod repair;
pub mod repair_fs;
pub mod sealed;
pub mod trash;
pub mod tree;
pub mod txn;
pub mod undo;
pub mod vol;

pub use csum::CsumKind;
pub use dev::{BlockDev, DevInfo, Image, IoError, MemDisk, Op, RecordingDisk};
pub use format::{BlockPtr, Superblock};
pub use fs::{Fs, Value};
pub use items::{FileType, Inode, ROOT};
pub use tree::Key;

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
    /// No such file, folder, name or attribute.
    NotFound,
    /// The name is taken.
    Exists,
    /// A folder was needed.
    NotDir,
    /// A file was needed.
    IsDir,
    /// The folder still holds names.
    NotEmpty,
    /// The caller asked for something format 1 cannot hold: what it was.
    Invalid(&'static str),
}

impl From<IoError> for Error {
    fn from(_: IoError) -> Self {
        Error::Io
    }
}
