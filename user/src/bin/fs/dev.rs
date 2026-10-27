//! kaifs's disk: the `kai-data` partition through the PART_* calls. A 4 KiB
//! block is 8 sectors, and block 0 is the partition's first.

use kabi::handle::Handle;
use kabi::part::{PartInfo, SECTOR, VOLATILE_CACHE, WRITABLE};
use kaifs::{BLOCK, Block, BlockDev, DevInfo, IoError};
use user::sys;

const SECTORS: u64 = BLOCK as u64 / SECTOR;

pub struct PartDev {
    part: Handle,
    info: DevInfo,
}

impl PartDev {
    /// A disk the kernel cannot make durable ([`WRITABLE`] clear) is reported as
    /// a volatile cache without FLUSH, so kaifs mounts it read-only.
    pub fn new(part: Handle, info: &PartInfo) -> PartDev {
        let writable = info.flags & WRITABLE != 0;
        PartDev {
            part,
            info: DevInfo {
                blocks: info.sectors / SECTORS,
                volatile_cache: info.flags & VOLATILE_CACHE != 0 || !writable,
                flush: writable,
            },
        }
    }
}

impl BlockDev for PartDev {
    fn info(&self) -> DevInfo {
        self.info
    }

    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        let sector = addr.checked_mul(SECTORS).ok_or(IoError)?;
        sys::part_read(self.part, sector, buf).map_err(|_| IoError)?;
        Ok(())
    }

    fn write(&mut self, addr: u64, data: &Block) -> Result<(), IoError> {
        let sector = addr.checked_mul(SECTORS).ok_or(IoError)?;
        sys::part_write(self.part, sector, data).map_err(|_| IoError)?;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        sys::part_flush(self.part).map_err(|_| IoError)?;
        Ok(())
    }
}
