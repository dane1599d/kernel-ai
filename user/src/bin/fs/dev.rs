//! kaifs's disk: the `kai-data` partition through the PART_* calls, a run of up
//! to 128 KiB a call ([`fsserve::batch`]). A 4 KiB block is 8 sectors, and
//! block 0 is the partition's first.

use core::sync::atomic::{AtomicU32, Ordering};
use fsserve::batch::RunDev;
use kabi::handle::Handle;
use kabi::part::{PartInfo, SECTOR, VOLATILE_CACHE, WRITABLE};
use kaifs::{BLOCK, DevInfo, IoError};
use user::{logln, sys};

const SECTORS: u64 = BLOCK as u64 / SECTOR;

/// Failures logged so far: the first few, then each power of two, so a dead
/// disk that fails every call does not fill the log.
static FAILED: AtomicU32 = AtomicU32::new(0);

/// Whether the `count`th failure (from 1) is logged.
fn logged(count: u32) -> bool {
    count <= 8 || count.is_power_of_two()
}

/// Logs a failed PART_* call with what the kernel answered, the sector and the
/// length, so a disk error that reaches `fs` as a bare [`IoError`] names its
/// cause.
fn failed(call: &str, sector: u64, len: usize, error: kabi::Error) -> IoError {
    let count = FAILED.fetch_add(1, Ordering::Relaxed).saturating_add(1);
    if logged(count) {
        logln!("fs: part {call} failed: {error:?} (sector {sector}, {len} bytes; failure {count})");
    }
    IoError
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
        sys::part_read(self.part, sector, buf)
            .map_err(|error| failed("read", sector, buf.len(), error))?;
        Ok(())
    }

    fn write_run(&mut self, addr: u64, data: &[u8]) -> Result<(), IoError> {
        let sector = addr.checked_mul(SECTORS).ok_or(IoError)?;
        sys::part_write(self.part, sector, data)
            .map_err(|error| failed("write", sector, data.len(), error))?;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        sys::part_flush(self.part).map_err(|error| failed("flush", 0, 0, error))?;
        Ok(())
    }
}
