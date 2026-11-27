//! The installer's plan (ADR 0008 §1 and §12, P8 row 2): given a target disk's
//! size, the ordered writes that lay out ADR 0008 §1's five partitions from the
//! running image. The ESP and `kai-r` are copied from the running disk, and the
//! running system's committed slot, A or B, becomes the install's `kai-a`.
//! `kai-b` is left empty, and `kai-data` is formatted as a release image's is.
//! The disk's and partitions' GUIDs come from an install id, as `xtask
//! release-image --image-id` derives them, so every install has its own.
//!
//! [`plan`] decides; [`apply`] writes. The order is what makes a cut safe:
//! 1. the target's old GPT, both copies, and its protective MBR are zeroed, so
//!    the disk no longer boots as anything;
//! 2. every partition's contents are written, then read back: the ESP and
//!    `kai-r` must match the running disk's, and `kai-a` must hash to the
//!    committed slot's SHA-256 and hold its `boot/slot` record;
//! 3. the protective MBR and both entry arrays;
//! 4. the backup header; and last
//! 5. the primary header.
//!
//! Each step ends with a flush. Until step 4 lands no GPT header on the disk is
//! valid, so firmware finds no partition on it and the kernel no `kai-data`.
//! After step 4 every partition is whole, so even a firmware that restores the
//! primary from the backup boots a complete install. There is never a half
//! disk that boots as Kernel AI.

#![no_std]

extern crate alloc;

mod apply;
mod data;

pub use apply::{Current, Disk, IoError, Window, apply, current};
pub use data::{FOLDERS, format_data};

use alloc::format;
use alloc::vec::Vec;
use diskmap::{Guid, Spec, Table};
use sha2::{Digest, Sha256};

/// The only sector size the plan writes: GPT here and in `diskmap` is 512-byte.
pub const SECTOR: u32 = 512;
const MIB_SECTORS: u64 = 2048;
/// The first partition's LBA: 1 MiB in, where every tool expects it.
pub const FIRST_LBA: u64 = MIB_SECTORS;
/// ADR 0008 §1's sizes in MiB, in disk order: the ESP, `kai-r`, `kai-a`,
/// `kai-b`. 2,368 MiB before `kai-data`.
pub const PARTITION_MIB: [u64; 4] = [64, 256, 1024, 1024];
/// The partitions' GPT names, in disk order.
pub const NAMES: [&str; 5] = [
    "EFI system partition",
    "kai-r",
    "kai-a",
    "kai-b",
    "kai-data",
];
/// Their type GUIDs, in the same order.
pub const KINDS: [Guid; 5] = [
    diskmap::ESP_TYPE,
    diskmap::KAI_R_TYPE,
    diskmap::KAI_A_TYPE,
    diskmap::KAI_B_TYPE,
    diskmap::KAI_DATA_TYPE,
];
/// Sectors of `kai-b`'s start that are zeroed, so no old volume is found there.
pub const CLEARED: u64 = MIB_SECTORS;
/// `kai-data`'s fewest kaifs blocks. kaifs formats 64 (`MIN_BLOCKS`), but
/// [`format_data`]'s folders need 90 today; this leaves room above that.
pub const DATA_MIN_BLOCKS: u64 = 128;
/// Sectors in one kaifs block.
const BLOCK_SECTORS: u64 = (kaifs::BLOCK / SECTOR as usize) as u64;

/// A version-4-shaped GUID from `label`, in GPT's byte order.
pub fn guid(label: &str) -> [u8; 16] {
    let digest = Sha256::digest(label.as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out[7] = (out[7] & 0x0F) | 0x40;
    out[8] = (out[8] & 0x3F) | 0x80;
    out
}

/// A disk's, a partition's or a volume's own GUID: from the image's or the
/// install's id and what it is the GUID of (`disk`, `partition kai-a`).
pub fn unique(id: &str, what: &str) -> [u8; 16] {
    guid(&format!("{id} {what}"))
}

/// Whether `id` may seed an install's GUIDs: 1 to 64 printable ASCII characters
/// with no space, as `--image-id` takes.
pub fn id_ok(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_graphic())
}

/// Why a disk cannot be installed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The target's sectors are not 512 bytes.
    SectorSize(u32),
    /// The target is smaller than the layout and `kai-data`'s minimum, which
    /// needs this many sectors.
    TooSmall { need: u64 },
    /// The install id is not one [`id_ok`] takes.
    BadId,
    /// The running image cannot be copied: a partition missing, or one larger
    /// than its place on the target, or a volume that does not fit it.
    Source(&'static str),
    /// The target has the running disk's GUID: it is the running disk, or
    /// another disk written from the same image (which shares its GUIDs).
    RunningDisk,
    /// The target is not the size the plan was made for, or its table is not
    /// the one the plan was made over (and the user agreed to wipe).
    Target,
    /// A copy did not read back as it should; nothing past it was written.
    Verify(&'static str),
    /// `diskmap` refused the layout.
    Layout(diskmap::Error),
    /// A disk read, write or flush failed.
    Io,
    /// kaifs refused to format `kai-data`, at the named step.
    Data(&'static str, kaifs::Error),
}

/// One partition the plan lays out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub kind: Guid,
    pub guid: Guid,
    pub first: u64,
    pub last: u64,
    pub name: &'static str,
}

impl Part {
    pub fn sectors(&self) -> u64 {
        self.last - self.first + 1
    }
}

/// One step of a plan, in the order [`apply`] takes them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Zeros over `sectors` from `lba`.
    Zero { lba: u64, sectors: u64 },
    /// The FAT32 volume at the source's `from`, as many sectors as its boot
    /// sector says and at most `most`, to the target's `to`.
    Copy { from: u64, to: u64, most: u64 },
    /// The copy [`Step::Copy`] with the same fields made, read back from the
    /// target and checked as `expect` says.
    Check {
        from: u64,
        to: u64,
        most: u64,
        expect: Expect,
    },
    /// `kai-data` formatted over `sectors` from `lba` ([`format_data`]).
    Format {
        lba: u64,
        sectors: u64,
        uuid: [u8; 16],
        now: u64,
    },
    /// `bytes`, whole sectors, at `lba`.
    Write { lba: u64, bytes: Vec<u8> },
    /// Everything written before is durable.
    Flush,
}

/// What a copy must read back as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// The same bytes as the source.
    Same,
    /// The committed slot's image: its SHA-256, and a `boot/slot` record with
    /// its `release_seq`.
    Slot { sha256: [u8; 32], release_seq: u32 },
}

/// A target disk's install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The target's size in sectors.
    pub sectors: u64,
    /// What the target held when the plan was made, which the install wipes:
    /// what the device shows the user before it asks to go on. [`apply`]
    /// refuses a target that no longer holds this.
    pub wipes: Current,
    pub disk_guid: Guid,
    /// The five partitions, in disk order.
    pub parts: Vec<Part>,
    pub steps: Vec<Step>,
}

/// What to install, and where.
#[derive(Clone, Copy, Debug)]
pub struct Target<'a> {
    /// The target's size in sectors of `sector_size` bytes.
    pub sectors: u64,
    pub sector_size: u32,
    /// Seeds the disk's, the partitions' and `kai-data`'s GUIDs. The device
    /// makes a random one per install.
    pub id: &'a str,
    /// `kai-data`'s clock at format, in seconds.
    pub now: u64,
}

/// The running system's committed slot, as the boot record gives it: the
/// install's `kai-a` is a copy of it. Where `sha256` comes from is in decision
/// row `2026-11-27-installer-plan`.
#[derive(Clone, Copy, Debug)]
pub struct Committed {
    /// `diskmap::KAI_A_TYPE` or `diskmap::KAI_B_TYPE`.
    pub kind: Guid,
    /// The slot image's SHA-256, over its volume's sectors.
    pub sha256: [u8; 32],
    /// The release its `boot/slot` record names.
    pub release_seq: u32,
}

/// The running disk: its table, and which slot is committed.
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    pub table: &'a Table,
    pub committed: Committed,
}

/// Where `kai-data` starts: right after `kai-b`.
pub fn data_first() -> u64 {
    FIRST_LBA + PARTITION_MIB.iter().sum::<u64>() * MIB_SECTORS
}

/// The fewest sectors a target may have: the layout, `kai-data`'s minimum
/// ([`DATA_MIN_BLOCKS`]) and the backup GPT.
pub fn minimum_sectors() -> u64 {
    let data = DATA_MIN_BLOCKS * BLOCK_SECTORS;
    // last_usable(sectors) = sectors - 2 - ENTRY_SECTORS must reach the data's end.
    data_first() + data + 1 + diskmap::ENTRY_SECTORS
}

/// The partitions for a target of `sectors`. `kai-data` takes the rest, in
/// whole kaifs blocks.
fn parts(sectors: u64, id: &str) -> Vec<Part> {
    let mut first = FIRST_LBA;
    let mut out = Vec::new();
    for (n, name) in NAMES.iter().enumerate() {
        let count = match PARTITION_MIB.get(n) {
            Some(mib) => mib * MIB_SECTORS,
            None => {
                let room = diskmap::last_usable(sectors) + 1 - first;
                room - room % BLOCK_SECTORS
            }
        };
        out.push(Part {
            kind: KINDS[n],
            guid: Guid(unique(id, &format!("partition {name}"))),
            first,
            last: first + count - 1,
            name,
        });
        first += count;
    }
    out
}

/// The plan that installs the running disk `source` to `target`, whose table
/// now is `current` ([`current`] reads it). Nothing is read or written:
/// [`apply`] does that. A target with the running disk's GUID is refused,
/// so another disk written from the same image is refused too.
pub fn plan(target: Target<'_>, source: Source<'_>, current: &Current) -> Result<Plan, Error> {
    if target.sector_size != SECTOR {
        return Err(Error::SectorSize(target.sector_size));
    }
    if !id_ok(target.id) {
        return Err(Error::BadId);
    }
    let sectors = target.sectors;
    if matches!(current, Current::Gpt(t) if t.disk_guid == source.table.disk_guid) {
        return Err(Error::RunningDisk);
    }
    if sectors < minimum_sectors() {
        return Err(Error::TooSmall {
            need: minimum_sectors(),
        });
    }
    let parts = parts(sectors, target.id);
    let disk_guid = Guid(unique(target.id, "disk"));
    let specs: Vec<Spec<'_>> = parts
        .iter()
        .map(|p| Spec {
            kind: p.kind,
            guid: p.guid,
            first: p.first,
            last: p.last,
            name: p.name,
        })
        .collect();
    let gpt = diskmap::layout(sectors, disk_guid, &specs).map_err(Error::Layout)?;
    let [mbr, primary, array, backup_array, backup] =
        <[_; 5]>::try_from(gpt).map_err(|_| Error::Layout(diskmap::Error::BadLayout))?;

    let tail = 1 + diskmap::ENTRY_SECTORS;
    let mut steps = Vec::new();
    // 1. No old table survives: the MBR, the primary and the backup.
    steps.push(Step::Zero {
        lba: 0,
        sectors: diskmap::FIRST_USABLE,
    });
    steps.push(Step::Zero {
        lba: sectors - tail,
        sectors: tail,
    });
    steps.push(Step::Flush);
    // 2. The contents: the ESP, R, and the committed slot as kai-a.
    let committed = source.committed;
    if committed.kind != diskmap::KAI_A_TYPE && committed.kind != diskmap::KAI_B_TYPE {
        return Err(Error::Source("the committed slot is not A or B"));
    }
    let kinds = [diskmap::ESP_TYPE, diskmap::KAI_R_TYPE, committed.kind];
    let expects = [
        Expect::Same,
        Expect::Same,
        Expect::Slot {
            sha256: committed.sha256,
            release_seq: committed.release_seq,
        },
    ];
    let mut checks = Vec::new();
    for (n, (kind, expect)) in kinds.iter().zip(expects).enumerate() {
        let from = source
            .table
            .find(*kind)
            .ok_or(Error::Source("the running disk lacks a partition"))?;
        if from.sectors() > parts[n].sectors() {
            return Err(Error::Source(
                "a running partition is larger than its place",
            ));
        }
        let (from, to, most) = (from.first, parts[n].first, from.sectors());
        steps.push(Step::Copy { from, to, most });
        checks.push(Step::Check {
            from,
            to,
            most,
            expect,
        });
    }
    steps.push(Step::Zero {
        lba: parts[3].first,
        sectors: CLEARED,
    });
    let data = &parts[4];
    // Block 0 is outside kaifs and holds `kai-modelhint 1` (ADR 0004 §8): an
    // old install's must not name a model this disk lacks.
    steps.push(Step::Zero {
        lba: data.first,
        sectors: BLOCK_SECTORS,
    });
    steps.push(Step::Format {
        lba: data.first,
        sectors: data.sectors(),
        uuid: unique(target.id, "kai-data volume"),
        now: target.now,
    });
    steps.push(Step::Flush);
    // Read back once durable: nothing past a bad copy is written.
    steps.extend(checks);
    // 3-5. The table, the primary header last.
    for (lba, bytes) in [mbr, array, backup_array] {
        steps.push(Step::Write { lba, bytes });
    }
    steps.push(Step::Flush);
    for (lba, bytes) in [backup, primary] {
        steps.push(Step::Write { lba, bytes });
        steps.push(Step::Flush);
    }
    Ok(Plan {
        sectors,
        wipes: current.clone(),
        disk_guid,
        parts,
        steps,
    })
}
