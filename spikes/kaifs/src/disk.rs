//! The `BlockDev` trait and three disks: a sparse in-memory disk, a recording disk
//! that logs every write and flush, and an `Image`, a disk as a crash left it (a
//! base plus some of the logged writes, possibly torn).

use crate::{BLOCK, Block};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoError;

/// What kaifs needs of a disk (ADR 0004 §1): 4 KiB blocks, a write that may stay
/// in a volatile cache, and a flush that makes every earlier write durable.
pub trait BlockDev {
    fn blocks(&self) -> u64;
    fn read(&self, addr: u64, buf: &mut Block) -> Result<(), IoError>;
    fn write(&mut self, addr: u64, data: &Block) -> Result<(), IoError>;
    fn flush(&mut self) -> Result<(), IoError>;
}

/// A sparse disk: unwritten blocks read as zeros.
#[derive(Clone, Default)]
pub struct MemDisk {
    blocks: u64,
    data: BTreeMap<u64, Rc<Block>>,
}

impl MemDisk {
    pub fn new(blocks: u64) -> Self {
        Self {
            blocks,
            data: BTreeMap::new(),
        }
    }

    pub fn get(&self, addr: u64) -> Option<&Rc<Block>> {
        self.data.get(&addr)
    }

    fn put(&mut self, addr: u64, block: Rc<Block>) {
        self.data.insert(addr, block);
    }
}

impl BlockDev for MemDisk {
    fn blocks(&self) -> u64 {
        self.blocks
    }

    fn read(&self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        if addr >= self.blocks {
            return Err(IoError);
        }
        match self.data.get(&addr) {
            Some(block) => buf.copy_from_slice(&block[..]),
            None => buf.fill(0),
        }
        Ok(())
    }

    fn write(&mut self, addr: u64, data: &Block) -> Result<(), IoError> {
        if addr >= self.blocks {
            return Err(IoError);
        }
        self.put(addr, Rc::new(*data));
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        Ok(())
    }
}

/// One logged operation.
#[derive(Clone)]
pub enum Op {
    Write(u64, Rc<Block>),
    Flush,
}

/// A disk that applies every write at once and logs it, so the crash harness can
/// rebuild what a crash at any point could have left.
#[derive(Clone, Default)]
pub struct RecordingDisk {
    pub disk: MemDisk,
    pub log: Vec<Op>,
    /// Writes and flushes since the disk was made, for the write-amplification
    /// figures.
    pub writes: u64,
    pub flushes: u64,
}

impl RecordingDisk {
    pub fn new(blocks: u64) -> Self {
        Self {
            disk: MemDisk::new(blocks),
            ..Self::default()
        }
    }

    /// Forgets the log: the disk as it is now is the base of later crash states.
    pub fn checkpoint(&mut self) {
        self.log.clear();
    }
}

impl BlockDev for RecordingDisk {
    fn blocks(&self) -> u64 {
        self.disk.blocks
    }

    fn read(&self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        self.disk.read(addr, buf)
    }

    fn write(&mut self, addr: u64, data: &Block) -> Result<(), IoError> {
        if addr >= self.disk.blocks {
            return Err(IoError);
        }
        let block = Rc::new(*data);
        self.disk.put(addr, block.clone());
        self.log.push(Op::Write(addr, block));
        self.writes += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.log.push(Op::Flush);
        self.flushes += 1;
        Ok(())
    }
}

/// A disk as a crash left it: `base` with `overlay` on top. Writes go to the
/// overlay, so a mount may write without touching the base.
pub struct Image<'a> {
    pub base: &'a MemDisk,
    pub overlay: BTreeMap<u64, Rc<Block>>,
}

impl<'a> Image<'a> {
    pub fn new(base: &'a MemDisk) -> Self {
        Self {
            base,
            overlay: BTreeMap::new(),
        }
    }

    /// Applies one logged write.
    pub fn apply(&mut self, addr: u64, block: &Rc<Block>) {
        self.overlay.insert(addr, block.clone());
    }

    /// Applies a write torn after `bytes`: the first `bytes` new, the rest as the
    /// block was before.
    pub fn apply_torn(&mut self, addr: u64, block: &Block, bytes: usize) {
        let mut old: Box<Block> = Box::new([0; BLOCK]);
        let _ = self.read(addr, &mut old);
        old[..bytes].copy_from_slice(&block[..bytes]);
        self.overlay.insert(addr, Rc::new(*old));
    }
}

impl BlockDev for Image<'_> {
    fn blocks(&self) -> u64 {
        self.base.blocks
    }

    fn read(&self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        match self.overlay.get(&addr) {
            Some(block) => {
                buf.copy_from_slice(&block[..]);
                Ok(())
            }
            None => self.base.read(addr, buf),
        }
    }

    fn write(&mut self, addr: u64, data: &Block) -> Result<(), IoError> {
        if addr >= self.base.blocks {
            return Err(IoError);
        }
        self.overlay.insert(addr, Rc::new(*data));
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        Ok(())
    }
}
