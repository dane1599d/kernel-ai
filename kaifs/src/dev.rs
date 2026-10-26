//! The block device below kaifs (ADR 0004 §1), and the disks the tests use: a
//! sparse in-memory disk, a recording disk that logs every write and flush, and an
//! [`Image`], a disk as a crash left it.

use crate::{BLOCK, Block};
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoError;

/// What a device says about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DevInfo {
    /// Its size in 4 KiB blocks.
    pub blocks: u64,
    /// Writes may sit in a volatile cache until a flush.
    pub volatile_cache: bool,
    /// The device honours FLUSH.
    pub flush: bool,
}

impl DevInfo {
    /// Whether a commit can be made durable here. A volatile cache that cannot be
    /// flushed cannot, so such a device is refused read-write (ADR 0004 §3); a
    /// device with neither a cache nor FLUSH is write-through, as VIRTIO 1.3
    /// §5.2.5.1 allows.
    pub fn durable(&self) -> bool {
        !self.volatile_cache || self.flush
    }
}

/// What kaifs needs of a disk: 4 KiB blocks, writes that may stay in a volatile
/// cache, and a flush that makes every write completed before it durable. Writes
/// inside one commit go to free blocks, so their order does not matter.
pub trait BlockDev {
    fn info(&self) -> DevInfo;
    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError>;
    fn write(&mut self, addr: u64, data: &Block) -> Result<(), IoError>;
    fn flush(&mut self) -> Result<(), IoError>;
}

/// A device borrowed for a while: what lets a mount run over a disk the caller
/// keeps.
impl<T: BlockDev + ?Sized> BlockDev for &mut T {
    fn info(&self) -> DevInfo {
        (**self).info()
    }

    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        (**self).read(addr, buf)
    }

    fn write(&mut self, addr: u64, data: &Block) -> Result<(), IoError> {
        (**self).write(addr, data)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        (**self).flush()
    }
}

/// A sparse disk in memory: unwritten blocks read as zeros. Blocks are shared, so
/// a copy costs little and a crash state built from a log holds no copies.
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

    /// Sets block `addr` to `block`, without copying it.
    pub fn put(&mut self, addr: u64, block: Rc<Block>) {
        self.data.insert(addr, block);
    }

    fn read_into(&self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        if addr >= self.blocks {
            return Err(IoError);
        }
        match self.data.get(&addr) {
            Some(block) => buf.copy_from_slice(&block[..]),
            None => buf.fill(0),
        }
        Ok(())
    }
}

impl BlockDev for MemDisk {
    fn info(&self) -> DevInfo {
        DevInfo {
            blocks: self.blocks,
            volatile_cache: false,
            flush: true,
        }
    }

    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        self.read_into(addr, buf)
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

/// One logged device operation.
#[derive(Clone)]
pub enum Op {
    Write(u64, Rc<Block>),
    Flush,
}

/// A disk with a write-back cache that applies every write at once and logs it,
/// so the crash enumerator can rebuild what a crash at any point could leave.
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
    fn info(&self) -> DevInfo {
        DevInfo {
            blocks: self.disk.blocks,
            volatile_cache: true,
            flush: true,
        }
    }

    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        self.disk.read_into(addr, buf)
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

/// A disk as a crash left it: `base` with some writes on top. A mount may write
/// to it without touching the base.
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
    /// block was.
    pub fn apply_torn(&mut self, addr: u64, block: &Block, bytes: usize) {
        let mut torn = [0; BLOCK];
        let _ = self.read(addr, &mut torn);
        torn[..bytes].copy_from_slice(&block[..bytes]);
        self.overlay.insert(addr, Rc::new(torn));
    }
}

impl BlockDev for Image<'_> {
    fn info(&self) -> DevInfo {
        self.base.info()
    }

    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        match self.overlay.get(&addr) {
            Some(block) => {
                buf.copy_from_slice(&block[..]);
                Ok(())
            }
            None => self.base.read_into(addr, buf),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recording_disk_logs_and_an_image_layers_over_a_base() {
        let mut disk = RecordingDisk::new(16);
        let block = [7; BLOCK];
        disk.write(3, &block).unwrap();
        disk.flush().unwrap();
        assert_eq!((disk.writes, disk.flushes, disk.log.len()), (1, 1, 2));
        assert!(disk.write(16, &block).is_err());
        let base = disk.disk.clone();
        let mut image = Image::new(&base);
        image.apply_torn(3, &[9; BLOCK], 512);
        let mut buf = [0; BLOCK];
        image.read(3, &mut buf).unwrap();
        assert_eq!((buf[0], buf[511], buf[512]), (9, 9, 7));
        image.read(4, &mut buf).unwrap();
        assert_eq!(buf, [0; BLOCK]);
    }

    #[test]
    fn a_cache_without_flush_cannot_be_made_durable() {
        let info = |volatile_cache, flush| DevInfo {
            blocks: 1,
            volatile_cache,
            flush,
        };
        assert!(info(true, true).durable());
        assert!(info(false, false).durable());
        assert!(!info(true, false).durable());
    }
}
