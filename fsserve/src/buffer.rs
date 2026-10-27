//! What keeps 7 KiB messages from costing a 128 KiB record each. kaifs stores a
//! file in records of [`CHUNK`] bytes, and a write into one rewrites it whole, so
//! a file sent in [`fsproto::MAX_DATA`] pieces would be written about 18 times
//! over, and inside an action none of the copies could be reused before the
//! commit.
//! - Inside an action, consecutive writes to one file join a pending run, and
//!   only whole records go to kaifs as the run passes their ends. Any other
//!   request writes the run out first, so every reader sees it; a run that fails
//!   to write out drops the action, as a failed write would have.
//! - Reads come through a one-record cache, dropped by any change.

use super::{Server, error};
use alloc::vec::Vec;
use fsproto::Error;
use kaifs::BlockDev;
use kaifs::items::CHUNK;

/// Writes not yet given to kaifs: `data` at `off` in object `object`.
pub(crate) struct Pending {
    volume: u64,
    object: u64,
    off: u64,
    data: Vec<u8>,
}

/// One record of a file as kaifs last read it.
pub(crate) struct Cached {
    volume: u64,
    object: u64,
    chunk: u64,
    bytes: Vec<u8>,
}

impl<D: BlockDev> Server<D> {
    /// Writes `data` at `off` in `object` of the active volume.
    pub(crate) fn write(&mut self, object: u64, off: u64, data: &[u8]) -> Result<(), Error> {
        let volume = self.fs().volume();
        if self.owner.is_none() {
            return self.fs().write_at(object, off, data).map_err(error);
        }
        match &mut self.pending {
            Some(p) if (p.volume, p.object) == (volume, object) && p.end() == Some(off) => {
                p.data.extend_from_slice(data);
            }
            _ => {
                self.write_out()?;
                self.pending = Some(Pending {
                    volume,
                    object,
                    off,
                    data: data.to_vec(),
                });
            }
        }
        // Every whole record the run now reaches goes to kaifs; the rest waits.
        let p = self.pending.as_mut().expect("just set");
        let end = p.end().ok_or(Error::Invalid)?;
        let cut = end / CHUNK * CHUNK;
        if cut > p.off {
            let rest = p.data.split_off((cut - p.off) as usize);
            let whole = core::mem::replace(&mut p.data, rest);
            let at = core::mem::replace(&mut p.off, cut);
            if let Err(e) = self.fs().write_at(object, at, &whole) {
                self.drop_changes();
                return Err(error(e));
            }
        }
        Ok(())
    }

    /// Gives the pending run to kaifs; on failure the action is dropped.
    pub(crate) fn write_out(&mut self) -> Result<(), Error> {
        let Some(p) = self.pending.take() else {
            return Ok(());
        };
        let fs = self.fs();
        let volume = fs.volume();
        let written = fs
            .set_volume(p.volume)
            .and_then(|()| fs.write_at(p.object, p.off, &p.data));
        let _ = fs.set_volume(volume);
        if let Err(e) = written {
            self.drop_changes();
            return Err(error(e));
        }
        Ok(())
    }

    /// Up to `len` bytes of `object` from `off`, short at its end.
    pub(crate) fn read(
        &mut self,
        object: u64,
        off: u64,
        len: usize,
        size: u64,
    ) -> Result<Vec<u8>, Error> {
        let volume = self.fs().volume();
        let end = off.saturating_add(len as u64).min(size);
        let mut out = Vec::new();
        let mut at = off;
        while at < end {
            let chunk = at / CHUNK * CHUNK;
            let hit = self
                .cache
                .as_ref()
                .is_some_and(|c| (c.volume, c.object, c.chunk) == (volume, object, chunk));
            if !hit {
                let bytes = self
                    .fs()
                    .read_at(object, chunk, CHUNK as usize)
                    .map_err(error)?;
                self.cache = Some(Cached {
                    volume,
                    object,
                    chunk,
                    bytes,
                });
            }
            let bytes = &self.cache.as_ref().expect("just filled").bytes;
            let from = (at - chunk) as usize;
            let to = ((end - chunk) as usize).min(bytes.len());
            if from >= to {
                break;
            }
            out.extend_from_slice(&bytes[from..to]);
            at += (to - from) as u64;
        }
        Ok(out)
    }
}

impl Pending {
    fn end(&self) -> Option<u64> {
        self.off.checked_add(self.data.len() as u64)
    }
}
