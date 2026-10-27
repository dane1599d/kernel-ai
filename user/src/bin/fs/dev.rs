//! kaifs's disk: the `kai-data` partition through the PART_* calls, a run of up
//! to 128 KiB a call ([`fsserve::batch`]). A 4 KiB block is 8 sectors, and
//! block 0 is the partition's first.

use core::sync::atomic::{AtomicU64, Ordering};
use fsserve::batch::RunDev;
use kabi::handle::Handle;
use kabi::part::{PartInfo, SECTOR, VOLATILE_CACHE, WRITABLE};
use kaifs::{BLOCK, DevInfo, IoError};
use user::sys;

const SECTORS: u64 = BLOCK as u64 / SECTOR;

/// Each PART_* call's count and time since [`take`]: S04-2's breakdown.
static CALLS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
const READ: usize = 0;
const WRITE: usize = 2;
const FLUSH: usize = 4;

/// PART_READ, PART_WRITE and PART_FLUSH since the last call: (count, ns) each.
pub fn take() -> [(u64, u64); 3] {
    let at = |i: usize| CALLS[i].swap(0, Ordering::Relaxed);
    [READ, WRITE, FLUSH].map(|i| (at(i), at(i + 1)))
}

/// Times `call` as PART_* call `which`.
fn timed<T>(which: usize, call: impl FnOnce() -> T) -> T {
    let start = sys::time_ns();
    let out = call();
    CALLS[which].fetch_add(1, Ordering::Relaxed);
    CALLS[which + 1].fetch_add(sys::time_ns().saturating_sub(start), Ordering::Relaxed);
    out
}

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

impl RunDev for PartDev {
    fn info(&self) -> DevInfo {
        self.info
    }

    fn read_run(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let sector = addr.checked_mul(SECTORS).ok_or(IoError)?;
        timed(READ, || sys::part_read(self.part, sector, buf)).map_err(|_| IoError)?;
        Ok(())
    }

    fn write_run(&mut self, addr: u64, data: &[u8]) -> Result<(), IoError> {
        let sector = addr.checked_mul(SECTORS).ok_or(IoError)?;
        timed(WRITE, || sys::part_write(self.part, sector, data)).map_err(|_| IoError)?;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        timed(FLUSH, || sys::part_flush(self.part)).map_err(|_| IoError)?;
        Ok(())
    }
}
