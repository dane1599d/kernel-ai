//! The idle slot as a download writes it (ADR 0008 section 12): `kainet`'s verified
//! download calls a [`Partial`] to read back what it wrote before, write a chunk
//! once it matches its signed hash, record how many are done and, at the end, move
//! the target into place. For a slot there is no moving: it is already where it
//! goes, so the end is the read-back the stage event asks for (ADR 0008 section 4):
//! the whole slot is read and hashed, and only a slot that reads back as signed is
//! written.
//!
//! The slot and the checkpoint are the caller's ([`SlotIo`], [`Checkpoint`]): in
//! the OS `SLOT_WRITE` and a record in `/system/updates`; in the tests memory.

use crate::plan::Release;
use alloc::vec;
use kainet::download::{DownloadError, Partial};
use sha2::{Digest, Sha256};

/// What the store reports when the slot reads back as other than it was written.
pub const READ_BACK: &str = "the slot reads back other than the signed image";
/// How much of the slot one read-back step reads: the kernel's `MAX_IO`.
const STEP: usize = 1 << 20;

/// The idle slot, by byte offset. A write may be a partial last sector: the
/// implementation pads it with zeros.
pub trait SlotIo {
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), &'static str>;
    /// Fills `buf` from `offset`.
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), &'static str>;
    /// Makes what was written durable.
    fn flush(&mut self) -> Result<(), &'static str>;
}

/// How far a download of one image got: the image it is for, and how many chunks
/// are checked and written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub sha256: [u8; 32],
    pub done: usize,
}

/// Where the mark is kept between runs, and across a reboot. A `save` is durable
/// when it returns, and replaces the mark whole: after a crash the old mark or
/// the new.
pub trait Checkpoint {
    fn load(&mut self) -> Result<Option<Mark>, &'static str>;
    fn save(&mut self, mark: Option<Mark>) -> Result<(), &'static str>;
}

/// The idle slot with its checkpoint, for `release`.
pub struct SlotPartial<'a> {
    slot: &'a mut dyn SlotIo,
    checkpoint: &'a mut dyn Checkpoint,
    release: &'a Release,
}

impl<'a> SlotPartial<'a> {
    pub fn new(
        slot: &'a mut dyn SlotIo,
        checkpoint: &'a mut dyn Checkpoint,
        release: &'a Release,
    ) -> Self {
        Self {
            slot,
            checkpoint,
            release,
        }
    }

    /// Drops the mark: a download that ended in a way that taints what it wrote
    /// (the list was wrong) resumes from nothing.
    pub fn forget(&mut self) -> Result<(), &'static str> {
        self.checkpoint.save(None)
    }
}

impl Partial for SlotPartial<'_> {
    /// The chunks the last run wrote, for this image only: a mark for another
    /// image (a newer release was published since) is dropped, and the download
    /// starts over, writing over the slot.
    fn checkpoint(&mut self) -> Result<usize, DownloadError> {
        match self.checkpoint.load().map_err(DownloadError::Store)? {
            Some(mark) if mark.sha256 == self.release.sha256 => Ok(mark.done),
            Some(_) => {
                self.checkpoint.save(None).map_err(DownloadError::Store)?;
                Ok(0)
            }
            None => Ok(0),
        }
    }

    /// The slot is flushed first, so a mark never says more than the slot holds.
    fn record(&mut self, done: usize) -> Result<(), DownloadError> {
        self.slot.flush().map_err(DownloadError::Store)?;
        let mark = Mark {
            sha256: self.release.sha256,
            done,
        };
        self.checkpoint
            .save(Some(mark))
            .map_err(DownloadError::Store)
    }

    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DownloadError> {
        self.slot.write(offset, bytes).map_err(DownloadError::Store)
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), DownloadError> {
        self.slot.read(offset, buf).map_err(DownloadError::Store)
    }

    /// The idle slot is `updated`'s alone, and [`SlotPartial::finish`] reads
    /// it all back and hashes it itself: `download`'s own read-back would read
    /// it twice.
    fn held_alone(&self) -> bool {
        true
    }

    /// Reads the whole slot back and hashes it: it must be the signed image.
    fn finish(&mut self) -> Result<(), DownloadError> {
        self.slot.flush().map_err(DownloadError::Store)?;
        let mut hash = Sha256::new();
        let mut buf = vec![0u8; STEP];
        let mut at = 0;
        while at < self.release.length {
            let want = (self.release.length - at).min(STEP as u64) as usize;
            self.slot
                .read(at, &mut buf[..want])
                .map_err(DownloadError::Store)?;
            hash.update(&buf[..want]);
            at += want as u64;
        }
        let read: [u8; 32] = hash.finalize().into();
        match read == self.release.sha256 {
            true => Ok(()),
            false => Err(DownloadError::Store(READ_BACK)),
        }
    }
}
