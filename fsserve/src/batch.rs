//! Fewer, larger disk calls (S04-2's profile, #262): kaifs moves one 4 KiB
//! block at a time, and each `PART_*` call costs about 120 µs under KVM, so a
//! 32 MiB transfer spent a second a phase in calls alone. [`Batched`] gathers
//! contiguous writes into one run of up to [`RUN_BLOCKS`] and reads ahead on
//! sequential reads.
//!
//! It is a write-back cache to kaifs: gathered writes reach the disk only at
//! [`BlockDev::flush`] (or when a run fills or a write lands elsewhere), and
//! every read sees them first. So it always reports a volatile cache, and kaifs
//! always flushes, whatever the disk under it says about its own.

use alloc::vec::Vec;
use kaifs::{BLOCK, Block, BlockDev, DevInfo, IoError};

/// The most blocks one disk call moves: 128 KiB, `PART_*`'s limit.
pub const RUN_BLOCKS: usize = 32;

/// A disk that moves whole runs of blocks in one call.
pub trait RunDev {
    fn info(&self) -> DevInfo;
    /// Reads `buf.len() / BLOCK` blocks from `addr`.
    fn read_run(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), IoError>;
    /// Writes `data.len() / BLOCK` blocks at `addr`.
    fn write_run(&mut self, addr: u64, data: &[u8]) -> Result<(), IoError>;
    fn flush(&mut self) -> Result<(), IoError>;
}

/// Blocks starting at `at`, held in memory.
struct Run {
    at: u64,
    bytes: Vec<u8>,
}

impl Run {
    fn blocks(&self) -> u64 {
        (self.bytes.len() / BLOCK) as u64
    }

    fn holds(&self, addr: u64) -> bool {
        addr >= self.at && addr < self.at + self.blocks()
    }

    fn block(&mut self, addr: u64) -> &mut [u8] {
        let from = (addr - self.at) as usize * BLOCK;
        &mut self.bytes[from..from + BLOCK]
    }
}

pub struct Batched<D> {
    dev: D,
    /// Writes not yet on the disk: one contiguous run.
    pending: Option<Run>,
    /// What the last sequential read brought in beyond the block asked for.
    ahead: Option<Run>,
    /// The block after the last one read, to spot a sequential reader.
    next_read: u64,
}

impl<D: RunDev> Batched<D> {
    pub fn new(dev: D) -> Self {
        Batched {
            dev,
            pending: None,
            ahead: None,
            next_read: u64::MAX,
        }
    }

    pub fn dev(&mut self) -> &mut D {
        &mut self.dev
    }

    /// Puts the gathered writes on the disk.
    fn drain(&mut self) -> Result<(), IoError> {
        match self.pending.take() {
            Some(run) => self.dev.write_run(run.at, &run.bytes),
            None => Ok(()),
        }
    }
}

impl<D: RunDev> BlockDev for Batched<D> {
    fn info(&self) -> DevInfo {
        let info = self.dev.info();
        DevInfo {
            volatile_cache: info.volatile_cache || info.flush,
            ..info
        }
    }

    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        if let Some(run) = self.pending.as_mut().filter(|r| r.holds(addr)) {
            buf.copy_from_slice(run.block(addr));
            return Ok(());
        }
        let sequential = addr == self.next_read;
        self.next_read = addr + 1;
        if let Some(run) = self.ahead.as_mut().filter(|r| r.holds(addr)) {
            buf.copy_from_slice(run.block(addr));
            return Ok(());
        }
        let left = self.dev.info().blocks.saturating_sub(addr);
        let count = match sequential {
            true => (RUN_BLOCKS as u64).min(left).max(1) as usize,
            false => 1,
        };
        let mut bytes = alloc::vec![0u8; count * BLOCK];
        self.dev.read_run(addr, &mut bytes)?;
        buf.copy_from_slice(&bytes[..BLOCK]);
        // What is still pending is newer than the disk: it wins.
        let mut run = Run { at: addr, bytes };
        if let Some(pending) = &self.pending {
            for block in pending.at..pending.at + pending.blocks() {
                if run.holds(block) {
                    let from = (block - pending.at) as usize * BLOCK;
                    run.block(block)
                        .copy_from_slice(&pending.bytes[from..from + BLOCK]);
                }
            }
        }
        self.ahead = (count > 1).then_some(run);
        Ok(())
    }

    fn write(&mut self, addr: u64, data: &Block) -> Result<(), IoError> {
        if let Some(run) = self.ahead.as_mut().filter(|r| r.holds(addr)) {
            run.block(addr).copy_from_slice(data);
        }
        if let Some(run) = self.pending.as_mut() {
            if run.holds(addr) {
                run.block(addr).copy_from_slice(data);
                return Ok(());
            }
            if addr == run.at + run.blocks() && run.blocks() < RUN_BLOCKS as u64 {
                run.bytes.extend_from_slice(data);
                return Ok(());
            }
        }
        self.drain()?;
        self.pending = Some(Run {
            at: addr,
            bytes: data.to_vec(),
        });
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.drain()?;
        self.dev.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kaifs::MemDisk;

    /// A [`MemDisk`] counting the calls it is given.
    struct Counted {
        disk: MemDisk,
        reads: usize,
        writes: usize,
        flushes: usize,
        /// Whether a flush found every write before it already on the disk.
        flushed: Vec<u64>,
    }

    impl RunDev for Counted {
        fn info(&self) -> DevInfo {
            DevInfo {
                volatile_cache: false,
                ..self.disk.info()
            }
        }

        fn read_run(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), IoError> {
            self.reads += 1;
            for (i, block) in buf.chunks_mut(BLOCK).enumerate() {
                let mut one = [0u8; BLOCK];
                self.disk.read(addr + i as u64, &mut one)?;
                block.copy_from_slice(&one);
            }
            Ok(())
        }

        fn write_run(&mut self, addr: u64, data: &[u8]) -> Result<(), IoError> {
            self.writes += 1;
            for (i, block) in data.chunks(BLOCK).enumerate() {
                self.disk
                    .write(addr + i as u64, block.try_into().unwrap())?;
                self.flushed.push(addr + i as u64);
            }
            Ok(())
        }

        fn flush(&mut self) -> Result<(), IoError> {
            self.flushes += 1;
            Ok(())
        }
    }

    fn batched() -> Batched<Counted> {
        Batched::new(Counted {
            disk: MemDisk::new(1024),
            reads: 0,
            writes: 0,
            flushes: 0,
            flushed: Vec::new(),
        })
    }

    fn block(byte: u8) -> Block {
        [byte; BLOCK]
    }

    #[test]
    fn contiguous_writes_go_down_as_runs_and_all_before_the_flush() {
        let mut b = batched();
        for addr in 0..70 {
            b.write(addr, &block(addr as u8)).unwrap();
        }
        // Two full runs went down as they filled; the rest waits for the flush.
        assert_eq!((b.dev().writes, b.dev().flushed.len()), (2, 64));
        b.flush().unwrap();
        assert_eq!((b.dev().writes, b.dev().flushes), (3, 1));
        assert_eq!(b.dev().flushed, (0..70).collect::<Vec<u64>>());
        // A write elsewhere ends the run it does not continue.
        b.write(500, &block(1)).unwrap();
        b.write(9, &block(2)).unwrap();
        assert_eq!(b.dev().writes, 4);
    }

    #[test]
    fn reads_see_the_writes_not_yet_on_the_disk() {
        let mut b = batched();
        b.write(5, &block(7)).unwrap();
        b.write(6, &block(8)).unwrap();
        let mut got = [0u8; BLOCK];
        b.read(6, &mut got).unwrap();
        assert_eq!(got, block(8));
        // A read-ahead that spans pending blocks shows them, not the disk.
        b.read(3, &mut got).unwrap();
        b.read(4, &mut got).unwrap();
        b.read(5, &mut got).unwrap();
        assert_eq!(got, block(7));
        assert_eq!(b.dev().writes, 0);
    }

    #[test]
    fn sequential_reads_come_in_runs_and_random_ones_one_block_at_a_time() {
        let mut b = batched();
        for addr in 0..64 {
            b.dev().disk.write(addr, &block(addr as u8)).unwrap();
        }
        let mut got = [0u8; BLOCK];
        for addr in 0..64 {
            b.read(addr, &mut got).unwrap();
            assert_eq!(got, block(addr as u8));
        }
        // The first read is not known to be sequential yet: 1 + 2 runs + 1.
        assert!(b.dev().reads <= 4, "{} reads", b.dev().reads);
        let before = b.dev().reads;
        for addr in [300, 17, 900] {
            b.read(addr, &mut got).unwrap();
        }
        assert_eq!(b.dev().reads, before + 3);
    }

    #[test]
    fn a_write_after_a_read_ahead_is_what_the_next_read_sees() {
        let mut b = batched();
        let mut got = [0u8; BLOCK];
        b.read(0, &mut got).unwrap();
        b.read(1, &mut got).unwrap();
        b.write(2, &block(9)).unwrap();
        b.flush().unwrap();
        b.read(2, &mut got).unwrap();
        assert_eq!(got, block(9));
    }

    #[test]
    fn it_always_reports_a_volatile_cache_so_kaifs_always_flushes() {
        assert!(batched().info().volatile_cache);
    }
}
