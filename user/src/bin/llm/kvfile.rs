//! **Interim** (S10-3; removed at P2 task 10, when the files move to
//! `/system/models/cache/`): llm's KV slots kept across boots, each as a
//! `kai-kv 1` file (`infer::segments`) in a fixed region of the `kai-cache`
//! partition, through MODEL_KV_LOAD and MODEL_KV_SAVE. Every call moves whole
//! sectors, up to [`KV_IO_BYTES`] at a time, so the reader and writer here buffer
//! what the engine streams.

use alloc::vec;
use alloc::vec::Vec;
use kabi::model::{KV_IO_BYTES, KV_REGION_BYTES, KV_SECTOR};
use user::sys;

/// Fills each buffer it is given from region `region`, in order: what
/// `Engine::restore_slot` reads from. False past the region or on a failed read.
pub fn reader(region: u64) -> impl FnMut(&mut [u8]) -> bool {
    let mut chunk: Vec<u8> = vec![0; KV_IO_BYTES as usize];
    // The chunk's offset in the region, and how much of it was handed out.
    let (mut offset, mut used) = (0u64, KV_IO_BYTES as usize);
    let mut loaded = false;
    move |out: &mut [u8]| {
        let mut done = 0;
        while done < out.len() {
            if used == chunk.len() {
                if loaded {
                    offset += KV_IO_BYTES;
                }
                if offset + KV_IO_BYTES > KV_REGION_BYTES
                    || sys::model_kv_load(region, offset, &mut chunk).is_err()
                {
                    return false;
                }
                (used, loaded) = (0, true);
            }
            let take = (out.len() - done).min(chunk.len() - used);
            out[done..done + take].copy_from_slice(&chunk[used..used + take]);
            (done, used) = (done + take, used + take);
        }
        true
    }
}

/// Writes what `Engine::save_slot` streams to region `region`, in whole sectors;
/// [`Writer::finish`] pads the last and makes it all durable.
pub struct Writer {
    region: u64,
    chunk: Vec<u8>,
    filled: usize,
    offset: u64,
    failed: bool,
}

impl Writer {
    pub fn new(region: u64) -> Self {
        Self {
            region,
            chunk: vec![0; KV_IO_BYTES as usize],
            filled: 0,
            offset: 0,
            failed: false,
        }
    }

    /// Takes `bytes`; false once a write failed or the region is full.
    pub fn write(&mut self, mut bytes: &[u8]) -> bool {
        while !bytes.is_empty() && !self.failed {
            let take = bytes.len().min(self.chunk.len() - self.filled);
            self.chunk[self.filled..self.filled + take].copy_from_slice(&bytes[..take]);
            (self.filled, bytes) = (self.filled + take, &bytes[take..]);
            if self.filled == self.chunk.len() {
                self.send(false);
            }
        }
        !self.failed
    }

    fn send(&mut self, flush: bool) {
        let len = (self.filled as u64).div_ceil(KV_SECTOR) * KV_SECTOR;
        self.chunk[self.filled..len as usize].fill(0);
        let fits = self.offset + len <= KV_REGION_BYTES;
        let sent = len == 0
            || (fits
                && sys::model_kv_save(
                    self.region,
                    self.offset,
                    &self.chunk[..len as usize],
                    flush,
                )
                .is_ok());
        self.failed |= !sent;
        (self.offset, self.filled) = (self.offset + len, 0);
    }

    /// Writes what is left and makes every write durable; false if any failed.
    pub fn finish(mut self) -> bool {
        if !self.failed {
            match self.filled {
                // The file ended with a whole chunk, which is still in the
                // buffer: its last sector again carries the flush.
                0 if self.offset >= KV_SECTOR => {
                    let last = &self.chunk[self.chunk.len() - KV_SECTOR as usize..];
                    let at = self.offset - KV_SECTOR;
                    self.failed |= sys::model_kv_save(self.region, at, last, true).is_err();
                }
                0 => self.failed = true,
                _ => self.send(true),
            }
        }
        !self.failed
    }
}
