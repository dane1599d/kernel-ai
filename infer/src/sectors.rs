//! A [`Region`] over a disk area that moves whole sectors, a bounded number of
//! bytes a call, with no call that tells its size and none that only flushes:
//! the OS's KV spill region (`kabi::model::KV_SPILL_REGION`) as llm reaches it
//! through MODEL_KV_LOAD and MODEL_KV_SAVE (agents OS plan F4).
//!
//! The OS answers the same way, NotFound, for a call past the area's end and for
//! a disk with no `kai-cache` partition at all. So the size is found once, when
//! the region is opened: a first sector past the end means no spill room, and
//! the slot regions are none of this one's business. After that, a call past the
//! end is a failed read or write like any other ([`crate::kvstore::Unkept::Failed`]
//! or a refused restore), never a sign that the partition is gone.

use crate::kvstore::Region;
use alloc::vec;
use alloc::vec::Vec;

/// Why a disk call did not move its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Miss {
    /// It reached past the area's end (or there is no area).
    PastEnd,
    /// Anything else.
    Failed,
}

/// The disk calls: `at` and the length are whole sectors, the length at most the
/// region's `max_io`.
pub trait Sectored {
    fn load(&mut self, at: u64, buf: &mut [u8]) -> Result<(), Miss>;
    /// Writes `bytes`, then makes every write so far durable when `flush`.
    fn save(&mut self, at: u64, bytes: &[u8], flush: bool) -> Result<(), Miss>;
}

/// The most sectors a probe looks for: 2^40 sectors is far past any disk the OS
/// runs on, and keeps the doubling from overflowing.
const MAX_SECTORS: u64 = 1 << 40;

/// `D` as a [`Region`] of the size found when it was opened.
pub struct SectorRegion<D> {
    disk: D,
    sector: u64,
    max_io: u64,
    size: u64,
    /// The last sector written and where, saved again with the flush.
    last: Option<(u64, Vec<u8>)>,
}

impl<D: Sectored> SectorRegion<D> {
    /// Finds the area's size by loading single sectors: doubling until one is
    /// past the end, then halving the gap. None when the first sector is past
    /// the end (no room), when a probe fails otherwise (no size to trust), or
    /// when `sector` or `max_io` cannot be used (`max_io` is whole sectors, at
    /// least one).
    pub fn open(mut disk: D, sector: u64, max_io: u64) -> Option<Self> {
        if sector == 0 || max_io < sector || !max_io.is_multiple_of(sector) {
            return None;
        }
        let mut probe = vec![0u8; sector as usize];
        let mut loads = |n: u64| disk.load(n * sector, &mut probe);
        // `ok` is a sector that loads, `end` the first known not to.
        let (mut ok, mut end) = (0u64, 1u64);
        loads(0).ok()?;
        loop {
            match loads(end) {
                Ok(()) if end < MAX_SECTORS => (ok, end) = (end, end * 2),
                Ok(()) => return None,
                Err(Miss::PastEnd) => break,
                Err(Miss::Failed) => return None,
            }
        }
        while end - ok > 1 {
            let mid = ok + (end - ok) / 2;
            match loads(mid) {
                Ok(()) => ok = mid,
                Err(Miss::PastEnd) => end = mid,
                Err(Miss::Failed) => return None,
            }
        }
        Some(Self {
            disk,
            sector,
            max_io,
            size: end * sector,
            last: None,
        })
    }

    /// Whole sectors inside the region.
    fn inside(&self, at: u64, len: usize) -> bool {
        let len = len as u64;
        at.is_multiple_of(self.sector)
            && len.is_multiple_of(self.sector)
            && at.checked_add(len).is_some_and(|end| end <= self.size)
    }
}

impl<D: Sectored> Region for SectorRegion<D> {
    fn block(&self) -> u64 {
        self.sector
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&mut self, at: u64, buf: &mut [u8]) -> bool {
        if !self.inside(at, buf.len()) {
            return false;
        }
        let mut offset = at;
        buf.chunks_mut(self.max_io as usize).all(|chunk| {
            let loaded = self.disk.load(offset, chunk).is_ok();
            offset += chunk.len() as u64;
            loaded
        })
    }

    fn write_at(&mut self, at: u64, bytes: &[u8]) -> bool {
        if !self.inside(at, bytes.len()) {
            return false;
        }
        let mut offset = at;
        let saved = bytes.chunks(self.max_io as usize).all(|chunk| {
            let saved = self.disk.save(offset, chunk, false).is_ok();
            offset += chunk.len() as u64;
            saved
        });
        if saved && !bytes.is_empty() {
            let tail = bytes.len() - self.sector as usize;
            self.last = Some((offset - self.sector, bytes[tail..].to_vec()));
        }
        saved
    }

    /// The disk has no flush on its own: the last sector written is saved again,
    /// with the flush.
    fn flush(&mut self) -> bool {
        match &self.last {
            Some((at, bytes)) => self.disk.save(*at, bytes, true).is_ok(),
            None => true,
        }
    }
}
