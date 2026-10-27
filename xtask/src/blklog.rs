//! The writable `kai-data` disk the storage cases boot with, and S04-3's check
//! of its blklogwrites log (ADR 0004, P2 task 1; first run on spike/blk-flush,
//! #97).
//!
//! The guest's `test=blk-flush` (kernel/src/blkflush_tests.rs) runs a raw
//! copy-on-write commit protocol shaped like kaifs's: each commit writes its data
//! blocks and a root block to free space, flushes, then writes its superblock to
//! slot `N mod 8` of both rings and flushes again. blklogwrites records every
//! write and flush the device completed, in order (QEMU's block/blklogwrites.c;
//! the log format is dm-log-writes'). Here the log is replayed to every flush
//! point and, inside each flush epoch, to every subset of the epoch's writes (all
//! of them up to 12 writes, 4,096 random ones above) and to each write torn at
//! every 512-byte step. Each state must mount, its chain must check, and the
//! generation it mounts must lie between the newest commit whose second flush
//! had completed and the newest whole superblock the state holds. QEMU's
//! blklogwrites is GPL-2.0-or-later and only runs here in CI; nothing of it is
//! linked: this reads the log it writes.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// The data disk and its log, in `target/`, which is QEMU's working directory.
pub const IMAGE: &str = "kai-data.img";
pub const LOG: &str = "kai-data.log";
/// The serial the guest finds the disk by.
pub const SERIAL: &str = "kai-data";
/// The disk's size: 64 MiB.
pub const DISK_BYTES: u64 = 64 << 20;
/// Commits the guest makes (kernel/src/blkflush_tests.rs).
pub const COMMITS: u64 = 200;

const BLOCK: usize = 4096;
const SECTOR: usize = 512;
const SECTORS_PER_BLOCK: u64 = (BLOCK / SECTOR) as u64;
const BLOCKS: u64 = DISK_BYTES / BLOCK as u64;
const SLOTS: u64 = 8;
/// Where a block's own checksum sits, in superblocks and roots.
const CSUM_AT: usize = BLOCK - 8;

/// dm-log-writes' magic, as QEMU writes it.
const LOG_MAGIC: u64 = 0x006a_7366_7773_6872;
const LOG_FLUSH: u64 = 1 << 0;
const LOG_FUA: u64 = 1 << 1;
const LOG_DISCARD: u64 = 1 << 2;
const LOG_MARK: u64 = 1 << 3;

const SB_MAGIC: &[u8; 8] = b"KAIRAWSB";
const ROOT_MAGIC: &[u8; 8] = b"KAIRAWRT";

const EXHAUSTIVE: usize = 12;
const RANDOM_SUBSETS: usize = 4096;

/// What the case expects of the replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// Every state is consistent: the commit protocol and its flushes hold.
    Clean,
    /// The guest skipped a flush on purpose (`blkbug=noflush`), so the replay
    /// must find violations: the harness can see a missing flush.
    Caught,
}

/// FNV-1a over little-endian 64-bit words, the guest's checksum
/// (kernel/src/blkflush_tests.rs). `bytes` is a whole number of words.
pub fn sum(bytes: &[u8]) -> u64 {
    let (words, _) = bytes.as_chunks::<8>();
    words.iter().fold(0xcbf2_9ce4_8422_2325, |hash, word| {
        (hash ^ u64::from_le_bytes(*word)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    bytes
        .get(at..at + 8)
        .map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
}

/// The two blocks of superblock slot `slot`: ring A from block 1, ring B in the
/// last 8 blocks. Block 0 stays outside, as in kaifs.
fn slot_blocks(slot: u64) -> [u64; 2] {
    [1 + slot, BLOCKS - SLOTS + slot]
}

/// How a case attaches `kai-data`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Data {
    /// On an NVMe controller instead of virtio-blk.
    pub nvme: bool,
    /// Behind blklogwrites, its log replayed once the case has passed.
    pub log: Option<Expect>,
}

/// Makes a zeroed (sparse) data disk and an empty log for QEMU to fill.
pub fn prepare(target: &Path) -> Result<(), String> {
    let image = fs::File::create(target.join(IMAGE)).map_err(|e| format!("{IMAGE}: {e}"))?;
    image
        .set_len(DISK_BYTES)
        .map_err(|e| format!("{IMAGE}: {e}"))?;
    fs::write(target.join(LOG), b"").map_err(|e| format!("{LOG}: {e}"))
}

/// The QEMU arguments: the disk with a write-back cache (write-cache=on, so the
/// device offers FLUSH, and NVMe reports a volatile write cache) and never
/// `-snapshot`; with `data.log`, behind blklogwrites with the log on a second file.
pub fn qemu_args(data: Data) -> Vec<String> {
    let mut args = vec![
        String::from("-blockdev"),
        format!("driver=file,filename={IMAGE},node-name=kai-data-file"),
    ];
    let node = match data.log {
        Some(_) => {
            args.extend([
                String::from("-blockdev"),
                format!("driver=file,filename={LOG},node-name=kai-data-log"),
                String::from("-blockdev"),
                String::from(
                    "driver=blklogwrites,node-name=kai-data,file=kai-data-file,log=kai-data-log,log-super-update-interval=1",
                ),
            ]);
            "kai-data"
        }
        None => "kai-data-file",
    };
    let device = match data.nvme {
        true => format!("nvme,drive={node},serial={SERIAL},write-cache=on"),
        false => format!("virtio-blk-pci,drive={node},serial={SERIAL},write-cache=on"),
    };
    args.extend([String::from("-device"), device]);
    args
}

struct Write {
    sector: u64,
    data: Vec<u8>,
}

impl Write {
    fn sectors(&self) -> u64 {
        (self.data.len() / SECTOR) as u64
    }

    fn blocks(&self) -> impl Iterator<Item = u64> + '_ {
        let first = self.sector / SECTORS_PER_BLOCK;
        let last = (self.sector + self.sectors().max(1) - 1) / SECTORS_PER_BLOCK;
        first..=last
    }
}

/// The writes between two flushes, and whether a flush ended them.
struct Epoch {
    writes: Vec<Write>,
    flushed: bool,
}

/// Reads the log into flush epochs.
fn parse(log: &[u8]) -> Result<Vec<Epoch>, String> {
    if u64_at(log, 0) != LOG_MAGIC || u64_at(log, 8) != 1 {
        return Err("blk-flush replay: the log has no blklogwrites superblock".into());
    }
    let entries = u64_at(log, 16);
    let sector_size =
        log.get(24..28)
            .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap_or([0; 4]))) as usize;
    if sector_size != SECTOR {
        return Err(format!(
            "blk-flush replay: log sector size {sector_size}, not 512"
        ));
    }
    let mut epochs = vec![Epoch {
        writes: Vec::new(),
        flushed: false,
    }];
    let mut at = SECTOR;
    for index in 0..entries {
        let (sector, count, flags) = (u64_at(log, at), u64_at(log, at + 8), u64_at(log, at + 16));
        if flags & !(LOG_FLUSH | LOG_FUA | LOG_DISCARD | LOG_MARK) != 0 || at + SECTOR > log.len() {
            return Err(format!("blk-flush replay: entry {index} is malformed"));
        }
        at += SECTOR;
        if flags & LOG_DISCARD != 0 {
            return Err(format!(
                "blk-flush replay: entry {index} is a discard, which the guest never sends"
            ));
        }
        if flags & LOG_FLUSH != 0 {
            if let Some(epoch) = epochs.last_mut() {
                epoch.flushed = true;
            }
            epochs.push(Epoch {
                writes: Vec::new(),
                flushed: false,
            });
            continue;
        }
        let len = count as usize * SECTOR;
        let data = log
            .get(at..at + len)
            .ok_or(format!("blk-flush replay: entry {index} is cut short"))?
            .to_vec();
        at += len;
        if let Some(epoch) = epochs.last_mut() {
            epoch.writes.push(Write { sector, data });
        }
    }
    Ok(epochs)
}

/// The disk at the last flush plus some writes over it, sector by sector.
struct View<'a> {
    base: &'a [u8],
    overlay: HashMap<u64, &'a [u8]>,
}

impl<'a> View<'a> {
    fn new(base: &'a [u8], writes: impl Iterator<Item = (&'a Write, u64)>) -> View<'a> {
        let mut overlay = HashMap::new();
        for (write, sectors) in writes {
            for k in 0..sectors {
                let at = k as usize * SECTOR;
                overlay.insert(write.sector + k, &write.data[at..at + SECTOR]);
            }
        }
        View { base, overlay }
    }

    fn touches(&self, block: u64) -> bool {
        (0..SECTORS_PER_BLOCK).any(|k| self.overlay.contains_key(&(block * SECTORS_PER_BLOCK + k)))
    }

    fn block(&self, block: u64) -> Vec<u8> {
        let mut out = vec![0; BLOCK];
        for k in 0..SECTORS_PER_BLOCK {
            let sector = block * SECTORS_PER_BLOCK + k;
            let at = sector as usize * SECTOR;
            let src = self
                .overlay
                .get(&sector)
                .copied()
                .unwrap_or_else(|| self.base.get(at..at + SECTOR).unwrap_or(&[0; SECTOR]));
            out[k as usize * SECTOR..(k as usize + 1) * SECTOR].copy_from_slice(src);
        }
        out
    }
}

/// A superblock slot that checks: its generation and root.
#[derive(Clone, Copy)]
struct Super {
    generation: u64,
    root: u64,
    root_sum: u64,
}

fn read_super(block: &[u8]) -> Option<Super> {
    (&block[..8] == SB_MAGIC && sum(&block[..CSUM_AT]) == u64_at(block, CSUM_AT)).then(|| Super {
        generation: u64_at(block, 8),
        root: u64_at(block, 16),
        root_sum: u64_at(block, 24),
    })
}

struct Replay {
    base: Vec<u8>,
    /// Roots, as (block, checksum), whose whole chain checked on `base`.
    verified: HashSet<(u64, u64)>,
    /// Every block in a verified chain: never to be written again.
    live: HashSet<u64>,
    /// The newest generation a flush has made durable.
    durable: u64,
    states: u64,
    violations: Vec<String>,
}

impl Replay {
    /// Mounts `view` as kaifs would: the newest slot whose own checksum and whose
    /// root's checksum verify. Then checks that root's whole chain.
    fn mount(&self, view: &View) -> Result<u64, String> {
        let mut supers: Vec<Super> = (0..SLOTS)
            .flat_map(slot_blocks)
            .filter_map(|block| read_super(&view.block(block)))
            .collect();
        supers.sort_by_key(|s| std::cmp::Reverse(s.generation));
        let Some(chosen) = supers
            .into_iter()
            .find(|s| sum(&view.block(s.root)) == s.root_sum)
        else {
            return Ok(0);
        };
        self.check_chain(view, chosen)?;
        Ok(chosen.generation)
    }

    fn check_chain(&self, view: &View, from: Super) -> Result<(), String> {
        let (mut root, mut root_sum, mut generation) = (from.root, from.root_sum, from.generation);
        loop {
            if self.verified.contains(&(root, root_sum)) && !view.touches(root) {
                return Ok(());
            }
            let block = view.block(root);
            if sum(&block) != root_sum
                || &block[..8] != ROOT_MAGIC
                || u64_at(&block, 8) != generation
            {
                return Err(format!(
                    "generation {generation}: its root at block {root} does not check"
                ));
            }
            let count = u64_at(&block, 32) as usize;
            for n in 0..count.min(200) {
                let (data, data_sum) = (u64_at(&block, 40 + 16 * n), u64_at(&block, 48 + 16 * n));
                if sum(&view.block(data)) != data_sum {
                    return Err(format!(
                        "generation {generation}: data block {data} does not check"
                    ));
                }
            }
            if generation == 1 {
                return Ok(());
            }
            (root, root_sum, generation) = (u64_at(&block, 16), u64_at(&block, 24), generation - 1);
        }
    }

    /// Checks one crash state: what it mounts lies in `lo..=hi`, exactly `hi` at a
    /// flush point.
    fn state(&mut self, view: &View, lo: u64, hi: u64, exact: bool, what: &dyn Fn() -> String) {
        self.states += 1;
        let bad = match self.mount(view) {
            Err(why) => Some(why),
            Ok(g) if g < lo => Some(format!("mounted generation {g}, older than durable {lo}")),
            Ok(g) if g > hi => Some(format!(
                "mounted generation {g}, newer than any whole superblock ({hi})"
            )),
            Ok(g) if exact && g != hi => {
                Some(format!("mounted generation {g} at a flush point, not {hi}"))
            }
            Ok(_) => None,
        };
        if let Some(why) = bad {
            if self.violations.len() < 20 {
                self.violations.push(format!("{}: {why}", what()));
            } else {
                self.violations.push(String::new());
            }
        }
    }

    /// Every crash state of one epoch, then the state at its flush.
    fn epoch(&mut self, number: usize, epoch: &Epoch, rng: &mut u64) {
        let writes = &epoch.writes;
        for write in writes {
            if let Some(block) = write.blocks().find(|b| self.live.contains(b)) {
                self.violations.push(format!(
                    "epoch {number}: a write over block {block}, which a durable commit holds"
                ));
            }
        }
        let lo = self.durable;
        let generation_of = |write: &Write| {
            let first = write.sector / SECTORS_PER_BLOCK;
            let is_slot = (0..SLOTS).flat_map(slot_blocks).any(|b| b == first);
            (is_slot
                && write.sector.is_multiple_of(SECTORS_PER_BLOCK)
                && write.sectors() == SECTORS_PER_BLOCK)
                .then(|| read_super(&write.data).map(|s| s.generation))
                .flatten()
        };
        let hi_of = |applied: &mut dyn Iterator<Item = &Write>| {
            applied.filter_map(generation_of).fold(lo, u64::max)
        };
        let n = writes.len();
        let masks: Vec<u64> = if n <= EXHAUSTIVE {
            (0..1u64 << n).collect()
        } else {
            let mut masks = vec![0, (1u64 << n.min(63)) - 1];
            for _ in 0..RANDOM_SUBSETS {
                *rng ^= *rng << 13;
                *rng ^= *rng >> 7;
                *rng ^= *rng << 17;
                masks.push(*rng & ((1u64 << n.min(63)) - 1));
            }
            masks
        };
        let base = std::mem::take(&mut self.base);
        for &mask in &masks {
            let chosen = || {
                writes
                    .iter()
                    .enumerate()
                    .filter(move |(i, _)| mask >> i & 1 == 1)
                    .map(|(_, w)| w)
            };
            let view = View::new(&base, chosen().map(|w| (w, w.sectors())));
            let hi = hi_of(&mut chosen());
            self.state(&view, lo, hi, false, &|| {
                format!("epoch {number}, writes {mask:#b}")
            });
        }
        // Each write torn at every 512-byte step, over none and over all the others.
        for (torn, write) in writes.iter().enumerate() {
            for keep in 1..write.sectors() {
                for others in [false, true] {
                    let applied = writes
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != torn && others)
                        .map(|(_, w)| (w, w.sectors()))
                        .chain(std::iter::once((write, keep)));
                    let view = View::new(&base, applied);
                    let hi = hi_of(
                        &mut writes
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| *i != torn && others)
                            .map(|(_, w)| w),
                    );
                    self.state(&view, lo, hi, false, &|| {
                        format!("epoch {number}, write {torn} torn after {keep} sectors")
                    });
                }
            }
        }
        self.base = base;
        // The flush point: every write in, and only now durable.
        for write in writes {
            let at = write.sector as usize * SECTOR;
            self.base[at..at + write.data.len()].copy_from_slice(&write.data);
        }
        if epoch.flushed {
            let hi = hi_of(&mut writes.iter());
            let base = std::mem::take(&mut self.base);
            let view = View::new(&base, std::iter::empty());
            self.state(&view, lo, hi, true, &|| {
                format!("epoch {number}, at its flush")
            });
            if let Ok(g) = self.mount(&view) {
                self.remember(&view, g);
                self.durable = self.durable.max(g);
            }
            self.base = base;
        }
    }

    /// Marks the chain of generation `g` on the flushed disk as verified and live.
    fn remember(&mut self, view: &View, g: u64) {
        let supers: Vec<Super> = (0..SLOTS)
            .flat_map(slot_blocks)
            .filter_map(|block| read_super(&view.block(block)))
            .filter(|s| s.generation == g)
            .collect();
        let Some(top) = supers.first().copied() else {
            return;
        };
        let (mut root, mut root_sum) = (top.root, top.root_sum);
        while root != 0 && self.verified.insert((root, root_sum)) {
            let block = view.block(root);
            self.live.insert(root);
            let count = u64_at(&block, 32) as usize;
            for n in 0..count.min(200) {
                self.live.insert(u64_at(&block, 40 + 16 * n));
            }
            (root, root_sum) = (u64_at(&block, 16), u64_at(&block, 24));
        }
    }
}

/// Replays the log and checks it; prints the figures.
pub fn check(target: &Path, expect: Expect) -> Result<(), String> {
    let log = fs::read(target.join(LOG)).map_err(|e| format!("{LOG}: {e}"))?;
    check_log(&log, expect, COMMITS)
}

/// [`check`] over the log's bytes, from a guest that made `commits` commits.
fn check_log(log: &[u8], expect: Expect, commits: u64) -> Result<(), String> {
    let epochs = parse(log)?;
    let writes: usize = epochs.iter().map(|e| e.writes.len()).sum();
    let flushes = epochs.iter().filter(|e| e.flushed).count();
    let mut replay = Replay {
        base: vec![0; DISK_BYTES as usize],
        verified: HashSet::new(),
        live: HashSet::new(),
        durable: 0,
        states: 0,
        violations: Vec::new(),
    };
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    for (number, epoch) in epochs.iter().enumerate() {
        replay.epoch(number, epoch, &mut rng);
    }
    let violations = replay.violations.len();
    println!(
        "blk-flush replay: {} log entries ({writes} writes, {flushes} flushes), {} flush epochs; {} crash states checked, {violations} violations; durable generation {} of {commits}",
        writes + flushes,
        epochs.len(),
        replay.states,
        replay.durable
    );
    for why in replay.violations.iter().filter(|why| !why.is_empty()) {
        println!("blk-flush replay: violation: {why}");
    }
    match expect {
        Expect::Clean if violations == 0 && replay.durable == commits => Ok(()),
        Expect::Clean => Err(format!(
            "FAIL: the replay found {violations} violations, durable {} of {commits}",
            replay.durable
        )),
        Expect::Caught if violations > 0 => {
            println!("blk-flush replay: the skipped flush was caught");
            Ok(())
        }
        Expect::Caught => {
            Err("FAIL: the guest skipped a flush and the replay found nothing".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_is_fnv1a_over_words() {
        assert_eq!(sum(&[]), 0xcbf2_9ce4_8422_2325);
        assert_ne!(sum(&[0; 8]), sum(&[1, 0, 0, 0, 0, 0, 0, 0]));
    }

    #[test]
    fn a_log_without_its_superblock_is_refused() {
        assert!(parse(&[0; 1024]).is_err());
    }

    fn put(block: &mut [u8], at: usize, value: u64) {
        block[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// The log the guest's `test=blk-flush` would leave, built on the host the way
    /// kernel/src/blkflush_tests.rs writes: `skip_flush` drops each commit's first
    /// flush, and `overwrite` makes commit 3 write over commit 1's root.
    fn guest_log(commits: u64, skip_flush: bool, overwrite: bool) -> Vec<u8> {
        let mut entries: Vec<(u64, u64, Vec<u8>)> = Vec::new();
        let mut write =
            |block: u64, data: Vec<u8>| entries.push((block * SECTORS_PER_BLOCK, 0, data));
        let mut flushes = Vec::new();
        let mut next = 1 + SLOTS;
        let (mut prev_root, mut prev_sum) = (0, 0);
        let mut order = Vec::new();
        for generation in 1..=commits {
            let count = 1 + generation % 8;
            let mut root = vec![0u8; BLOCK];
            root[..8].copy_from_slice(ROOT_MAGIC);
            put(&mut root, 8, generation);
            put(&mut root, 16, prev_root);
            put(&mut root, 24, prev_sum);
            put(&mut root, 32, count);
            for index in 0..count {
                let mut data = vec![0u8; BLOCK];
                put(&mut data, 0, generation);
                put(&mut data, 8, index);
                put(&mut data, 16, generation * 1000 + index);
                put(&mut root, 40 + 16 * index as usize, next);
                put(&mut root, 48 + 16 * index as usize, sum(&data));
                write(next, data);
                order.push(false);
                next += 1;
            }
            let root_at = if overwrite && generation == 3 {
                1 + SLOTS + 2
            } else {
                next
            };
            let root_sum = sum(&root);
            next += 1;
            write(root_at, root);
            order.push(false);
            if !skip_flush {
                flushes.push(order.len());
                order.push(true);
            }
            let mut superblock = vec![0u8; BLOCK];
            superblock[..8].copy_from_slice(SB_MAGIC);
            put(&mut superblock, 8, generation);
            put(&mut superblock, 16, root_at);
            put(&mut superblock, 24, root_sum);
            let own = sum(&superblock[..CSUM_AT]);
            put(&mut superblock, CSUM_AT, own);
            for block in slot_blocks(generation % SLOTS) {
                write(block, superblock.clone());
                order.push(false);
            }
            order.push(true);
            (prev_root, prev_sum) = (root_at, root_sum);
        }
        let mut log = vec![0u8; SECTOR];
        put(&mut log, 0, LOG_MAGIC);
        put(&mut log, 8, 1);
        put(&mut log, 16, order.len() as u64);
        log[24..28].copy_from_slice(&(SECTOR as u32).to_le_bytes());
        let mut writes = entries.into_iter();
        for is_flush in order {
            let mut header = vec![0u8; SECTOR];
            if is_flush {
                put(&mut header, 16, LOG_FLUSH);
                log.extend(header);
            } else if let Some((sector, _, data)) = writes.next() {
                put(&mut header, 0, sector);
                put(&mut header, 8, (data.len() / SECTOR) as u64);
                log.extend(header);
                log.extend(data);
            }
        }
        log
    }

    #[test]
    fn a_guest_that_flushes_in_order_replays_clean() {
        assert_eq!(
            check_log(&guest_log(12, false, false), Expect::Clean, 12),
            Ok(())
        );
    }

    #[test]
    fn a_skipped_flush_is_caught() {
        let log = guest_log(12, true, false);
        assert!(check_log(&log, Expect::Clean, 12).is_err());
        assert_eq!(check_log(&log, Expect::Caught, 12), Ok(()));
    }

    #[test]
    fn a_write_over_a_durable_block_is_caught() {
        assert!(check_log(&guest_log(12, false, true), Expect::Clean, 12).is_err());
    }
}
