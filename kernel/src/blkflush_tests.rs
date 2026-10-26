//! Spike S04-3 (ADR 0004), raw half, `test=blk-flush`: the writable `kai-data`
//! disk, behind QEMU's blklogwrites, takes 200 commits of a raw copy-on-write
//! protocol shaped like kaifs's (ADR 0004 §3). Commit N writes 1 to 8 data blocks
//! and a root block to free space, FLUSH, then its superblock to slot N mod 8 of
//! both rings, FLUSH. Nothing is ever written over a block a commit holds. xtask
//! then replays the log at every flush point and every crash state between
//! (xtask/src/blklog.rs, which shares this layout). With `blkbug=noflush` the
//! first flush of each commit is skipped, which the replay must catch. Never
//! merged.

use crate::block::{self, Device};
use crate::{hpet, log};
use alloc::vec;
use alloc::vec::Vec;

const SERIAL: &str = "kai-data";
const COMMITS: u64 = 200;
const BLOCK: usize = 4096;
const SECTORS_PER_BLOCK: u64 = 8;
/// 64 MiB, as xtask makes it.
const DISK_BLOCKS: u64 = 16_384;
/// Superblock slots per ring: ring A from block 1, ring B in the last 8 blocks.
const SLOTS: u64 = 8;
/// Where a superblock's own checksum sits.
const CSUM_AT: usize = BLOCK - 8;
const FIRST_DATA: u64 = 1 + SLOTS;
const MAX_DATA: u64 = 8;

/// FNV-1a over little-endian 64-bit words; `bytes` is a whole number of words.
fn sum(bytes: &[u8]) -> u64 {
    let (words, _) = bytes.as_chunks::<8>();
    words.iter().fold(0xcbf2_9ce4_8422_2325, |hash, word| {
        (hash ^ u64::from_le_bytes(*word)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn put(block: &mut [u8], at: usize, value: u64) {
    block[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn xorshift(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

fn data_block(generation: u64, index: u64) -> Vec<u8> {
    let mut block = vec![0u8; BLOCK];
    block[..8].copy_from_slice(b"KAIRAWDT");
    put(&mut block, 8, generation);
    put(&mut block, 16, index);
    let mut x = generation.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ index ^ 1;
    for word in block[24..].as_chunks_mut::<8>().0 {
        *word = xorshift(&mut x).to_le_bytes();
    }
    block
}

fn write_block(disk: &mut dyn Device, block: u64, bytes: &[u8]) -> Result<(), &'static str> {
    disk.write(block * SECTORS_PER_BLOCK, bytes)
}

fn timed_flush(disk: &mut dyn Device, times: &mut Vec<u64>) -> Result<(), &'static str> {
    let start = hpet::nanos();
    disk.flush()?;
    times.push(hpet::nanos() - start);
    Ok(())
}

/// The `p`th percentile of `times`, in microseconds.
fn percentile_us(times: &mut [u64], p: usize) -> u64 {
    times.sort_unstable();
    times[(times.len() * p / 100).min(times.len() - 1)] / 1_000
}

pub fn check(cmdline: &str) -> Result<(), &'static str> {
    let skip_flush = cmdline.split_whitespace().any(|w| w == "blkbug=noflush");
    block::with_disk(SERIAL, |disk: &mut dyn Device| run(disk, skip_flush))
        .ok_or("blk-flush: no disk with serial kai-data")?
}

fn run(disk: &mut dyn Device, skip_flush: bool) -> Result<(), &'static str> {
    if disk.read_only() {
        return Err("blk-flush: the kai-data disk is read-only");
    }
    if !disk.flushes() {
        // ADR 0004 §3: a write-back cache without FLUSH is refused read-write.
        return Err("blk-flush: the device offers no FLUSH; refused read-write");
    }
    if disk.sectors() != DISK_BLOCKS * SECTORS_PER_BLOCK {
        return Err("blk-flush: the kai-data disk is not 64 MiB");
    }
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    let mut next = FIRST_DATA;
    let (mut prev_root, mut prev_sum) = (0, 0);
    let mut commits = Vec::new();
    let mut flushes = Vec::new();
    for generation in 1..=COMMITS {
        let start = hpet::nanos();
        let count = 1 + xorshift(&mut rng) % MAX_DATA;
        let mut root = vec![0u8; BLOCK];
        root[..8].copy_from_slice(b"KAIRAWRT");
        put(&mut root, 8, generation);
        put(&mut root, 16, prev_root);
        put(&mut root, 24, prev_sum);
        put(&mut root, 32, count);
        for index in 0..count {
            let data = data_block(generation, index);
            write_block(disk, next, &data)?;
            put(&mut root, 40 + 16 * index as usize, next);
            put(&mut root, 48 + 16 * index as usize, sum(&data));
            next += 1;
        }
        let (root_at, root_sum) = (next, sum(&root));
        next += 1;
        write_block(disk, root_at, &root)?;
        if !skip_flush {
            timed_flush(disk, &mut flushes)?;
        }
        let mut superblock = vec![0u8; BLOCK];
        superblock[..8].copy_from_slice(b"KAIRAWSB");
        put(&mut superblock, 8, generation);
        put(&mut superblock, 16, root_at);
        put(&mut superblock, 24, root_sum);
        let own = sum(&superblock[..CSUM_AT]);
        put(&mut superblock, CSUM_AT, own);
        let slot = generation % SLOTS;
        write_block(disk, 1 + slot, &superblock)?;
        write_block(disk, DISK_BLOCKS - SLOTS + slot, &superblock)?;
        timed_flush(disk, &mut flushes)?;
        commits.push(hpet::nanos() - start);
        (prev_root, prev_sum) = (root_at, root_sum);
    }
    if sum(disk.read(prev_root * SECTORS_PER_BLOCK, SECTORS_PER_BLOCK)?) != prev_sum {
        return Err("blk-flush: the last root read back wrong");
    }
    log!(
        "blk-flush: {COMMITS} commits durable, {} blocks written{}",
        next - FIRST_DATA + 2 * COMMITS,
        if skip_flush {
            ", the first flush of each skipped (blkbug=noflush)"
        } else {
            ""
        }
    );
    let max = commits.iter().max().copied().unwrap_or(0) / 1_000;
    log!(
        "blk-flush: commit latency (data, root, FLUSH, both superblocks, FLUSH): p50 {} us, p99 {} us, max {max} us",
        percentile_us(&mut commits, 50),
        percentile_us(&mut commits, 99)
    );
    log!(
        "blk-flush: FLUSH latency over {} flushes: p50 {} us, p99 {} us",
        flushes.len(),
        percentile_us(&mut flushes, 50),
        percentile_us(&mut flushes, 99)
    );
    Ok(())
}
