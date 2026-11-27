//! What a target holds before an install, for the device to show the user
//! before it asks to go on, and for [`crate::apply`] to check it still holds.

use sha2::{Digest, Sha256};

use crate::apply::{Disk, ReadOnly};

const S: usize = crate::SECTOR as usize;

/// The sectors read from the start of a disk: 2 MiB, so a volume that a
/// zeroed table left at the usual 1 MiB start shows, with its first MiB.
pub const HEAD: u64 = 4096;
/// The sectors read from the end of a disk: 1 MiB, the backup GPT and more.
pub const TAIL: u64 = 2048;

/// What a target holds: what [`crate::plan`] records as
/// [`crate::Plan::wipes`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Current {
    /// No partition table or volume found: the first [`HEAD`] sectors and
    /// the last [`TAIL`] are all zero. The rest of the disk is not read, so
    /// this is not "empty".
    Blank,
    /// A valid GPT, read from the primary or the backup.
    Gpt(diskmap::Table),
    /// Something else: not blank, and not a GPT the installer reads.
    Other {
        /// Why it is not a GPT.
        why: &'static str,
        /// What the first sectors look like.
        found: Found,
        /// The SHA-256 of the sectors read, so [`crate::apply`] refuses
        /// another disk of the same size swapped in.
        print: [u8; 32],
    },
    /// The disk fails to read. [`crate::plan`] refuses it: there is nothing
    /// to show the user, and nothing to tell it from another such disk.
    ReadFails,
}

/// What the first sectors of a disk that is not blank and not a GPT look
/// like, from its magic bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// A LUKS header at sector 0.
    Luks,
    /// An ext2, ext3 or ext4 superblock at byte 1024.
    Ext,
    /// A btrfs superblock at 64 KiB.
    Btrfs,
    /// An ISO 9660 volume descriptor at 32 KiB.
    Iso9660,
    /// An NTFS boot sector at sector 0.
    Ntfs,
    /// An exFAT boot sector at sector 0.
    ExFat,
    /// A FAT boot sector at sector 0: a whole-disk FAT volume.
    Fat,
    /// A GPT header that fails its checks.
    GptHeader,
    /// An MBR: its four partitions' type bytes.
    Mbr([u8; 4]),
    /// Nonzero bytes, none of the above: a zeroed table's partitions left in
    /// place, for one.
    Unknown,
}

impl Current {
    /// The table, for a valid GPT.
    pub fn gpt(self) -> Option<diskmap::Table> {
        match self {
            Current::Gpt(t) => Some(t),
            _ => None,
        }
    }
}

/// The sectors [`found`] looks at: 0, 1, 2, 64 (32 KiB), 128 (64 KiB).
const LOOKED_AT: [u64; 5] = [0, 1, 2, 64, 128];

/// What `disk` holds now: what [`crate::plan`] takes as the target's
/// current contents.
pub fn current(disk: &mut dyn Disk) -> Current {
    let why = match diskmap::read(&mut ReadOnly(disk)) {
        Ok(table) => return Current::Gpt(table),
        Err(diskmap::Error::NoTable) => "no valid GPT header",
        Err(diskmap::Error::Read) => return Current::ReadFails,
        Err(diskmap::Error::Small) => "the disk is too small for a GPT",
        Err(diskmap::Error::BadPartitions) => "a GPT header names partitions that are not valid",
        Err(diskmap::Error::BadLayout) => "the GPT cannot be read",
    };
    let sectors = disk.sectors();
    let head = HEAD.min(sectors);
    let tail = sectors.saturating_sub(TAIL).max(head);
    let mut hash = Sha256::new();
    let mut zero = true;
    let mut looked = [[0u8; S]; LOOKED_AT.len()];
    let mut last = [0u8; S];
    let mut sector = [0u8; S];
    for lba in (0..head).chain(tail..sectors) {
        if disk.read(lba, &mut sector).is_err() {
            return Current::ReadFails;
        }
        hash.update(sector);
        zero &= sector == [0; S];
        if let Some(n) = LOOKED_AT.iter().position(|&at| at == lba) {
            looked[n] = sector;
        }
        if lba + 1 == sectors {
            last = sector;
        }
    }
    if zero {
        return Current::Blank;
    }
    other(why, found(&looked, &last), hash.finalize().into())
}

fn other(why: &'static str, found: Found, print: [u8; 32]) -> Current {
    Current::Other { why, found, print }
}

/// What the [`LOOKED_AT`] sectors and the last sector of a disk that is
/// neither blank nor a valid GPT look like.
fn found(looked: &[[u8; S]; LOOKED_AT.len()], last: &[u8; S]) -> Found {
    let [boot, header, ext, iso, btrfs] = looked;
    if boot.starts_with(b"LUKS\xba\xbe") {
        return Found::Luks;
    }
    // The ext superblock is at byte 1024, sector 2; its magic at offset 56.
    if ext[56..58] == [0x53, 0xEF] {
        return Found::Ext;
    }
    // btrfs: "_BHRfS_M" at offset 64 of its superblock at 64 KiB.
    if btrfs[64..72] == *b"_BHRfS_M" {
        return Found::Btrfs;
    }
    // ISO 9660: the first volume descriptor at 32 KiB, "CD001" at offset 1.
    if iso[1..6] == *b"CD001" {
        return Found::Iso9660;
    }
    // NTFS and exFAT name themselves at offset 3 of their boot sector, which
    // also carries 0x55AA, so they come before the MBR.
    if boot[3..11] == *b"NTFS    " {
        return Found::Ntfs;
    }
    if boot[3..11] == *b"EXFAT   " {
        return Found::ExFat;
    }
    let signed = boot[510..] == [0x55, 0xAA];
    if signed && (boot[82..87] == *b"FAT32" || boot[54..57] == *b"FAT") {
        return Found::Fat;
    }
    if header.starts_with(b"EFI PART") || last.starts_with(b"EFI PART") {
        return Found::GptHeader;
    }
    if signed {
        return Found::Mbr(core::array::from_fn(|i| boot[446 + 16 * i + 4]));
    }
    Found::Unknown
}
