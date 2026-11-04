//! **Interim** (S10-3; removed at P2 task 10, when the files move to
//! `/system/models/cache/`): llm's KV slots kept across boots, each as a
//! `kai-kv 1` file (`infer::segments`) in a fixed region of the `kai-cache`
//! partition, through MODEL_KV_LOAD and MODEL_KV_SAVE. Every call moves whole
//! sectors, up to [`KV_IO_MAX_BYTES`] at a time (llm asks for [`KV_IO_BYTES`]
//! unless a spike says otherwise), so the reader and writer here buffer what the
//! engine streams.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use kabi::model::{KV_IO_BYTES, KV_REGION_BYTES, KV_SECTOR};
use user::sys;

/// Fills each buffer it is given from region `region`, in order: what
/// `Engine::restore_slot` reads from. False past the region or on a failed read.
pub fn reader(region: u64) -> impl FnMut(&mut [u8]) -> bool {
    reader_with(region, KV_IO_BYTES as usize)
}

/// [`reader`] moving `size` bytes a call: a whole number of sectors, at most
/// `KV_IO_MAX_BYTES`.
pub fn reader_with(region: u64, size: usize) -> impl FnMut(&mut [u8]) -> bool {
    let step = size as u64;
    let mut chunk: Vec<u8> = vec![0; size];
    // The chunk's offset in the region, and how much of it was handed out.
    let (mut offset, mut used) = (0u64, size);
    let mut loaded = false;
    move |out: &mut [u8]| {
        let mut done = 0;
        while done < out.len() {
            if used == chunk.len() {
                if loaded {
                    offset += step;
                }
                if offset + step > KV_REGION_BYTES
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
/// [`Writer::finish`] pads the last and, unless the writer is not `durable`, makes
/// it all durable.
pub struct Writer {
    region: u64,
    /// Whether `finish` makes the file durable: a spill that only has to outlast a
    /// preemption does not need to.
    durable: bool,
    chunk: Vec<u8>,
    filled: usize,
    offset: u64,
    failed: bool,
}

impl Writer {
    pub fn new(region: u64) -> Self {
        Self::with(region, KV_IO_BYTES as usize, true)
    }

    /// A writer moving `size` bytes a call (a whole number of sectors, at most
    /// `KV_IO_MAX_BYTES`), durable at its end or not.
    pub fn with(region: u64, size: usize, durable: bool) -> Self {
        Self {
            region,
            durable,
            chunk: vec![0; size],
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

    /// Writes what is left and, if the writer is durable, makes every write durable;
    /// false if any failed.
    pub fn finish(mut self) -> bool {
        if self.failed {
            return false;
        }
        if !self.durable {
            if self.filled != 0 {
                self.send(false);
            }
            return !self.failed;
        }
        match self.filled {
            // The file ended with a whole chunk, which is still in the buffer: its
            // last sector again carries the flush.
            0 if self.offset >= KV_SECTOR => {
                let last = &self.chunk[self.chunk.len() - KV_SECTOR as usize..];
                let at = self.offset - KV_SECTOR;
                self.failed |= sys::model_kv_save(self.region, at, last, true).is_err();
            }
            0 => self.failed = true,
            _ => self.send(true),
        }
        !self.failed
    }
}

/// Where S13-1's measurement (`infer::preempt`) spills a slot: one region of the
/// `kai-cache` partition, through the same writer and reader the KV files use.
pub struct RegionSpill {
    region: u64,
    size: usize,
    durable: bool,
    writer: Option<Writer>,
}

impl RegionSpill {
    pub fn new(region: u64) -> Self {
        Self::with(region, KV_IO_BYTES as usize, true)
    }

    /// A spill that moves `size` bytes a call and is made durable at its end or not.
    pub fn with(region: u64, size: usize, durable: bool) -> Self {
        Self {
            region,
            size,
            durable,
            writer: None,
        }
    }
}

impl infer::preempt::Spill for RegionSpill {
    fn writer(&mut self) -> infer::preempt::Write<'_> {
        let writer = self
            .writer
            .insert(Writer::with(self.region, self.size, self.durable));
        Box::new(move |bytes| writer.write(bytes))
    }

    fn finish(&mut self) -> bool {
        self.writer.take().is_some_and(Writer::finish)
    }

    fn reader(&mut self) -> infer::preempt::Read<'_> {
        Box::new(reader_with(self.region, self.size))
    }
}

/// What `total` bytes cost to move to and from region `region` with no engine in
/// the way, `size` bytes a call, in nanoseconds of the guest's clock.
#[derive(Clone, Copy, Debug)]
pub struct RawIo {
    pub calls: u64,
    pub write_ns: u64,
    /// One sector written with the flush flag, after everything else.
    pub flush_ns: u64,
    pub read_ns: u64,
}

/// Writes `total` bytes (a whole number of `size`) to the region's start with no
/// flush, flushes once, and reads them back; `None` if a call fails.
pub fn raw_io(region: u64, size: usize, total: u64) -> Option<RawIo> {
    let mut buf: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let calls = total / size as u64;
    let step = size as u64;
    let started = sys::time_ns();
    for call in 0..calls {
        sys::model_kv_save(region, call * step, &buf, false).ok()?;
    }
    let write_ns = sys::time_ns() - started;
    let started = sys::time_ns();
    sys::model_kv_save(region, 0, &buf[..KV_SECTOR as usize], true).ok()?;
    let flush_ns = sys::time_ns() - started;
    let started = sys::time_ns();
    for call in 0..calls {
        sys::model_kv_load(region, call * step, &mut buf).ok()?;
    }
    let read_ns = sys::time_ns() - started;
    Some(RawIo {
        calls,
        write_ns,
        flush_ns,
        read_ns,
    })
}
