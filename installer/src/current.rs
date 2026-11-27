//! What a target holds before an install, for the device to show the user
//! before it asks to go on, and for [`crate::apply`] to check it still holds.

use sha2::{Digest, Sha256};

use crate::apply::{Disk, ReadOnly};

const S: usize = crate::SECTOR as usize;

/// What a target holds: what [`crate::plan`] records as
/// [`crate::Plan::wipes`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Current {
    /// Every sector of both GPT areas is zero: the MBR, the primary header
    /// and array, and the backup array and header. The rest of the disk is
    /// not read.
    Blank,
    /// A valid GPT, read from the primary or the backup.
    Gpt(diskmap::Table),
    /// Something else: not blank, and not a GPT the installer reads.
    Other {
        /// Why it is not a GPT.
        why: &'static str,
        /// What the first sectors look like.
        found: Found,
        /// The SHA-256 of both GPT areas, so [`crate::apply`] refuses
        /// another disk of the same size swapped in.
        print: [u8; 32],
    },
}

/// What the first sectors of a disk that is not blank and not a GPT look
/// like, from its magic bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// A LUKS header at sector 0.
    Luks,
    /// An ext2, ext3 or ext4 superblock at byte 1024.
    Ext,
    /// A FAT boot sector at sector 0: a whole-disk FAT volume.
    Fat,
    /// A GPT header that fails its checks.
    GptHeader,
    /// An MBR: its four partitions' type bytes.
    Mbr([u8; 4]),
    /// Nonzero bytes, none of the above.
    Unknown,
    /// The disk fails to read.
    ReadFails,
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

fn other(why: &'static str, found: Found, print: [u8; 32]) -> Current {
    Current::Other { why, found, print }
}

/// What `disk` holds now: what [`crate::plan`] takes as the target's
/// current contents.
pub fn current(disk: &mut dyn Disk) -> Current {
    let why = match diskmap::read(&mut ReadOnly(disk)) {
        Ok(table) => return Current::Gpt(table),
        Err(diskmap::Error::NoTable) => "no valid GPT header",
        Err(diskmap::Error::Read) => {
            return other("the disk fails to read", Found::ReadFails, [0; 32]);
        }
        Err(diskmap::Error::Small) => "the disk is too small for a GPT",
        Err(diskmap::Error::BadPartitions) => "a GPT header names partitions that are not valid",
        Err(diskmap::Error::BadLayout) => "the GPT cannot be read",
    };
    let sectors = disk.sectors();
    let areas = diskmap::FIRST_USABLE;
    let tail = sectors
        .saturating_sub(1 + diskmap::ENTRY_SECTORS)
        .max(areas);
    let mut hash = Sha256::new();
    let mut zero = true;
    let mut head = [[0u8; S]; 3];
    let mut last = [0u8; S];
    let mut sector = [0u8; S];
    for lba in (0..areas.min(sectors)).chain(tail..sectors) {
        if disk.read(lba, &mut sector).is_err() {
            return other("the disk fails to read", Found::ReadFails, [0; 32]);
        }
        hash.update(sector);
        zero &= sector == [0; S];
        if let Some(h) = head.get_mut(lba as usize) {
            *h = sector;
        }
        if lba + 1 == sectors {
            last = sector;
        }
    }
    if zero {
        return Current::Blank;
    }
    other(why, found(&head, &last), hash.finalize().into())
}

/// What sectors 0-2 and the last sector of a disk that is neither blank nor
/// a valid GPT look like.
fn found(head: &[[u8; S]; 3], last: &[u8; S]) -> Found {
    let [mbr, header, ext] = head;
    if mbr.starts_with(b"LUKS\xba\xbe") {
        return Found::Luks;
    }
    // The ext superblock is at byte 1024, sector 2; its magic at offset 56.
    if ext[56..58] == [0x53, 0xEF] {
        return Found::Ext;
    }
    let signed = mbr[510..] == [0x55, 0xAA];
    if signed && (mbr[82..87] == *b"FAT32" || mbr[54..57] == *b"FAT") {
        return Found::Fat;
    }
    if header.starts_with(b"EFI PART") || last.starts_with(b"EFI PART") {
        return Found::GptHeader;
    }
    if signed {
        return Found::Mbr(core::array::from_fn(|i| mbr[446 + 16 * i + 4]));
    }
    Found::Unknown
}
