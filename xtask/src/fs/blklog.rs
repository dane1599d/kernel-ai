//! QEMU's blklogwrites log (spike S04-3; P2 task 9's power cut): its reader and
//! a replayer that checks every crash state the log allows with kaifs itself.
//!
//! **The log** is dm-log-writes' format, as QEMU's block/blklogwrites.c writes it:
//! a superblock in sector 0 (magic, version 1, the entry count, the sector size),
//! then each entry in a sector of its own (sector, sector count, flags) followed
//! by its data. Flags say FLUSH, FUA, DISCARD or MARK. QEMU's driver is
//! GPL-2.0-or-later and runs only in CI; nothing of it is linked here: this reads
//! the file it writes.
//!
//! **The replay.** Writes inside the image's `kai-data` partition become kaifs
//! block writes, one per 4 KiB block, since a power cut may keep any of a
//! request's blocks; writes elsewhere (the GPT) are dropped. Each flush ends an
//! epoch. The generation a flush makes durable is the newest commit on the disk
//! once it completes. kaifs's crash enumerator then rebuilds every state the log
//! allows, with the superblock slots torn at every 512-byte step. Each state must
//! mount, pass `check()` and every volume's namespace and UNDO rules, and hold
//! a generation no older than the last one made durable. A FUA write is taken as
//! one that may be lost, which only adds states: kaifs never sends one.

use super::disk::Region;
use kaifs::check::{Cache, check};
use kaifs::crash::{Options, Oracle, Recording, Rng, Tally, enumerate};
use kaifs::ns_check::{check_namespace, check_tree};
use kaifs::{BLOCK, Block, CsumKind, Fs, Image, MemDisk, Op, format};
use std::rc::Rc;

pub const MAGIC: u64 = 0x006a_7366_7773_6872;
const VERSION: u64 = 1;
pub const FLUSH: u64 = 1 << 0;
pub const FUA: u64 = 1 << 1;
pub const DISCARD: u64 = 1 << 2;
pub const MARK: u64 = 1 << 3;
const KNOWN: u64 = FLUSH | FUA | DISCARD | MARK;

/// One entry of the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// `data` written at `sector` (in the log's sector size).
    Write {
        sector: u64,
        data: Vec<u8>,
        fua: bool,
    },
    Flush,
    Discard {
        sector: u64,
        sectors: u64,
    },
    Mark,
}

/// A whole log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Log {
    pub sector_size: usize,
    pub entries: Vec<Entry>,
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    let mut w = [0; 8];
    w.copy_from_slice(b.get(at..at.checked_add(8)?)?);
    Some(u64::from_le_bytes(w))
}

/// Reads a log. Every length is checked against what the file holds before it
/// is trusted.
pub fn parse(bytes: &[u8]) -> Result<Log, String> {
    let bad = |what: &str| format!("the blklogwrites log: {what}");
    if u64_at(bytes, 0) != Some(MAGIC) || u64_at(bytes, 8) != Some(VERSION) {
        return Err(bad("no dm-log-writes superblock"));
    }
    let count = u64_at(bytes, 16).ok_or_else(|| bad("a short superblock"))?;
    let size = bytes
        .get(24..28)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .ok_or_else(|| bad("a short superblock"))?;
    if !(512..=4096).contains(&size) || !size.is_power_of_two() {
        return Err(bad(&format!("a sector size of {size}")));
    }
    let mut entries = Vec::new();
    let mut at = size;
    for index in 0..count {
        let head = bytes
            .get(at..at + size)
            .ok_or_else(|| bad(&format!("entry {index} is past the end")))?;
        let (sector, sectors, flags) = (
            u64_at(head, 0).unwrap_or(0),
            u64_at(head, 8).unwrap_or(0),
            u64_at(head, 16).unwrap_or(0),
        );
        if flags & !KNOWN != 0 {
            return Err(bad(&format!("entry {index} has unknown flags {flags:#x}")));
        }
        at += size;
        let entry = match flags {
            f if f & FLUSH != 0 => Entry::Flush,
            f if f & DISCARD != 0 => Entry::Discard { sector, sectors },
            f if f & MARK != 0 => Entry::Mark,
            f => {
                let len = usize::try_from(sectors)
                    .ok()
                    .and_then(|n| n.checked_mul(size))
                    .filter(|&len| at.checked_add(len).is_some_and(|end| end <= bytes.len()))
                    .ok_or_else(|| bad(&format!("entry {index}'s data is past the end")))?;
                let data = bytes[at..at + len].to_vec();
                at += len;
                Entry::Write {
                    sector,
                    data,
                    fua: f & FUA != 0,
                }
            }
        };
        entries.push(entry);
    }
    Ok(Log {
        sector_size: size,
        entries,
    })
}

/// Writes a log as QEMU would: what the host tests replay.
#[cfg(test)]
pub fn encode(log: &Log) -> Vec<u8> {
    let size = log.sector_size;
    let mut out = vec![0u8; size];
    out[..8].copy_from_slice(&MAGIC.to_le_bytes());
    out[8..16].copy_from_slice(&VERSION.to_le_bytes());
    out[16..24].copy_from_slice(&(log.entries.len() as u64).to_le_bytes());
    out[24..28].copy_from_slice(&(size as u32).to_le_bytes());
    for entry in &log.entries {
        let mut head = vec![0u8; size];
        let (sector, sectors, flags, data): (u64, u64, u64, &[u8]) = match entry {
            Entry::Write { sector, data, fua } => (
                *sector,
                (data.len() / size) as u64,
                if *fua { FUA } else { 0 },
                data,
            ),
            Entry::Flush => (0, 0, FLUSH, &[]),
            Entry::Discard { sector, sectors } => (*sector, *sectors, DISCARD, &[]),
            Entry::Mark => (0, 0, MARK, &[]),
        };
        head[..8].copy_from_slice(&sector.to_le_bytes());
        head[8..16].copy_from_slice(&sectors.to_le_bytes());
        head[16..24].copy_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&head);
        out.extend_from_slice(data);
    }
    out
}

/// The log as kaifs's ops on `region`: whole 4 KiB block writes and flushes.
/// Writes outside the region are dropped; a write that covers part of a block
/// of it, or a discard there, is refused, as kaifs sends neither.
pub fn ops(log: &Log, region: Region) -> Result<Vec<Op>, String> {
    let size = log.sector_size as u64;
    let (start, end) = (region.offset, region.offset + region.blocks * BLOCK as u64);
    let mut out = Vec::new();
    for (index, entry) in log.entries.iter().enumerate() {
        match entry {
            Entry::Flush => out.push(Op::Flush),
            Entry::Mark => {}
            Entry::Discard { sector, sectors } => {
                let lo = sector.saturating_mul(size);
                let hi = sector.saturating_add(*sectors).saturating_mul(size);
                if lo < end && hi > start {
                    return Err(format!("entry {index}: a discard inside kai-data"));
                }
            }
            Entry::Write { sector, data, .. } => {
                let lo = sector
                    .checked_mul(size)
                    .ok_or(format!("entry {index}: a sector out of range"))?;
                let hi = lo + data.len() as u64;
                if hi <= start || lo >= end {
                    continue;
                }
                let aligned = (lo - start) % BLOCK as u64 == 0 && data.len() % BLOCK == 0;
                if lo < start || hi > end || !aligned {
                    return Err(format!(
                        "entry {index}: a write that is not whole kai-data blocks"
                    ));
                }
                for (i, chunk) in data.as_chunks::<BLOCK>().0.iter().enumerate() {
                    let addr = (lo - start) / BLOCK as u64 + i as u64;
                    out.push(Op::Write(addr, Rc::new(*chunk)));
                }
            }
        }
    }
    Ok(out)
}

/// For each flush, the log position after it and the generation it made
/// durable: the newest commit on the disk once it completes.
fn waited(base: &MemDisk, ops: &[Op]) -> Vec<(usize, u64)> {
    let mut disk = base.clone();
    let newest = |disk: &mut MemDisk| {
        format::read_rings(disk)
            .ok()
            .and_then(|found| found.first().map(|sb| sb.generation))
            .unwrap_or(0)
    };
    let mut out = vec![(0, newest(&mut disk))];
    for (i, op) in ops.iter().enumerate() {
        match op {
            Op::Write(addr, block) => disk.put(*addr, block.clone()),
            Op::Flush => out.push((i + 1, newest(&mut disk))),
        }
    }
    out
}

/// Checks each crash state as `xtask fs check` does, through the memo: it
/// mounts, `check()` passes, and every tree of every volume keeps the namespace
/// rules.
struct Checks {
    cache: Cache,
}

impl Checks {
    fn generation(&mut self, image: &mut Image<'_>) -> Result<u64, String> {
        let found = format::read_rings(image).map_err(|e| format!("rings: {e:?}"))?;
        let verifies = |image: &mut Image<'_>, ptr: &kaifs::BlockPtr| {
            ptr.is_null() || format::read_checked(image, CsumKind::Seahash64, ptr, "root").is_ok()
        };
        let sb = found
            .into_iter()
            .find(|sb| {
                !sb.volumes.is_null() && verifies(image, &sb.volumes) && verifies(image, &sb.alloc)
            })
            .ok_or("no superblock whose roots verify")?;
        let mut mounted = None;
        let report = match self.cache.mounted(&sb) {
            Some(report) => report,
            None => {
                let mut fs = Fs::mount(&mut *image).map_err(|e| format!("mount: {e:?}"))?;
                let report = check(&mut fs, &mut self.cache).map_err(|e| format!("check: {e}"))?;
                check_namespace(&mut fs, &mut self.cache).map_err(|e| format!("namespace: {e}"))?;
                mounted = Some(fs);
                report
            }
        };
        for (vol, trees) in &report.volumes {
            for tree in std::iter::once(&trees.live).chain(trees.snaps.values()) {
                let held = self.cache.namespace(&tree.root).is_some_and(|ns| {
                    ns.max_oid < tree.next_oid && ns.max_changed <= tree.generation
                });
                if held {
                    continue;
                }
                if mounted.is_none() {
                    mounted = Some(Fs::mount(&mut *image).map_err(|e| format!("mount: {e:?}"))?);
                }
                let fs = mounted.as_mut().expect("mounted above");
                check_tree(
                    fs,
                    &tree.root,
                    tree.next_oid,
                    tree.generation,
                    &mut self.cache,
                )
                .map_err(|e| format!("volume {vol}: {e}"))?;
            }
        }
        Ok(report.generation)
    }
}

impl Oracle for Checks {
    fn judge(&mut self, image: &mut Image<'_>, min: u64) -> Result<(), String> {
        self.cache.taint(image.overlay.keys().copied());
        match self.generation(image)? {
            g if g < min => Err(format!(
                "mounted generation {g}, older than the durable {min}"
            )),
            _ => Ok(()),
        }
    }

    fn durable(&mut self, written: &[u64]) {
        self.cache.forget(written);
    }
}

/// kaifs's blocks of `region` in `image`, the disk before the log began; blocks
/// of zeros are left out.
pub fn base(image: &[u8], region: Region) -> MemDisk {
    let mut disk = MemDisk::new(region.blocks);
    for addr in 0..region.blocks {
        let at = (region.offset + addr * BLOCK as u64) as usize;
        let Some(bytes) = image.get(at..at + BLOCK) else {
            break;
        };
        if bytes.iter().any(|&b| b != 0) {
            let mut block: Block = [0; BLOCK];
            block.copy_from_slice(bytes);
            disk.put(addr, Rc::new(block));
        }
    }
    disk
}

/// Replays `log` over `base` and checks every crash state it allows.
pub fn replay(base: &MemDisk, log: &Log, region: Region) -> Result<Tally, String> {
    let ops = ops(log, region)?;
    let waited = waited(base, &ops);
    let slots: Vec<u64> = format::all_slots(region.blocks).collect();
    let tear = |addr: u64| slots.contains(&addr);
    let recording = Recording {
        base,
        log: &ops,
        waited: &waited,
        tear: &tear,
    };
    let mut checks = Checks {
        cache: Cache::default(),
    };
    Ok(enumerate(
        &recording,
        &Options::default(),
        &mut checks,
        &mut Rng(1),
    ))
}
