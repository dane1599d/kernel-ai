//! `espfat` (ADR 0008 §12): reads files on the EFI system partition's FAT32
//! volume through a block device, and replaces an existing file's contents so
//! that a power cut at any point leaves either the old file or the new one. P8
//! needs this to write each slot's Limine from the device.
//!
//! This is our own code and not the fatfs crate, which ADR 0008 names. Without
//! std, fatfs 0.3.6 needs `core_io`, and `core_io`'s build script fails on our
//! nightly (decision row `2026-11-26-espfat-own-fat32`).
//!
//! What it does, and nothing more:
//! - It mounts FAT32 volumes with 512-byte sectors. It refuses FAT12, FAT16,
//!   exFAT and other sector sizes.
//! - It finds a file by path, matching long names and 8.3 names without regard
//!   to ASCII case, and reads it.
//! - It replaces an existing file's contents ([`Volume::replace`]). It never
//!   creates, renames or deletes a file, writes no name and never formats a
//!   volume, so it writes no long names at all.
//!
//! It trusts nothing it reads. A bad boot sector, a cluster chain that loops,
//! leaves the volume or does not fit its file's size, and a directory larger
//! than FAT allows are all refused with [`Error`], never a panic.
//!
//! Crash safety ([`Volume::replace`]). It assumes that one sector write is
//! atomic and that a flush makes every write before it durable. Between two
//! flushes, writes may land in any order, or not at all.
//! 1. The new contents go to clusters that every FAT copy marks free.
//! 2. Every FAT copy chains those clusters. Nothing names them yet.
//! 3. One sector write switches the directory entry's first cluster and size.
//! 4. Every FAT copy frees the old chain.
//! 5. FSInfo gets its free count back. The first phase set it to "unknown".
//!
//! A flush separates each phase. A cut before phase 3 is durable leaves the old
//! file, and after it, the new one. Either way, each FAT copy on its own reads
//! the same file. A cut can leave clusters marked used that no file names
//! (leaked). That costs space, never data.

#![no_std]

extern crate alloc;

mod bpb;
mod dir;
mod fat;
mod volume;

pub use bpb::Geometry;
pub use volume::Volume;

/// The only sector size espfat mounts.
pub const SECTOR: usize = 512;

/// A disk (or partition) of 512-byte sectors.
pub trait Disk {
    /// Reads sector `lba`.
    fn read(&mut self, lba: u64, out: &mut [u8; SECTOR]) -> Result<(), IoError>;
    /// Writes sector `lba`. The write may stay in a volatile cache until
    /// [`Disk::flush`].
    fn write(&mut self, lba: u64, data: &[u8; SECTOR]) -> Result<(), IoError>;
    /// Makes every write that completed before it durable.
    fn flush(&mut self) -> Result<(), IoError>;
}

impl<T: Disk + ?Sized> Disk for &mut T {
    fn read(&mut self, lba: u64, out: &mut [u8; SECTOR]) -> Result<(), IoError> {
        (**self).read(lba, out)
    }

    fn write(&mut self, lba: u64, data: &[u8; SECTOR]) -> Result<(), IoError> {
        (**self).write(lba, data)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        (**self).flush()
    }
}

/// The device failed a read, write or flush.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The device failed.
    Io,
    /// The boot sector does not describe a FAT32 volume that espfat mounts.
    NotFat32(&'static str),
    /// The volume's FAT or directories are not consistent.
    Corrupt(&'static str),
    /// The path is empty, or a component is empty, `.` or `..`.
    BadPath,
    /// No file or directory has that name.
    NotFound,
    /// The path names a directory where a file was wanted.
    IsDir,
    /// A component before the last names a file.
    NotDir,
    /// The file is larger than the caller's limit, or than FAT32 allows.
    TooLarge,
    /// Too few clusters are free in every FAT copy for the new contents.
    NoSpace,
}

impl From<IoError> for Error {
    fn from(_: IoError) -> Self {
        Error::Io
    }
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
