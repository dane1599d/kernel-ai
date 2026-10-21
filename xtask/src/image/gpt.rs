//! A GPT disk with one partition, the EFI system partition: a protective MBR, the
//! primary header and entry array, the partition, and the backup array and header
//! at the end. GUIDs are derived from a label, so the same inputs give the same
//! disk.

use super::SECTOR;
use sha2::{Digest, Sha256};

/// The ESP's partition type, C12A7328-F81F-11D2-BA4B-00A0C93EC93B, as stored.
pub const ESP_TYPE: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];
const ENTRIES: u32 = 128;
const ENTRY_BYTES: u32 = 128;
/// Sectors the entry array takes: 128 entries of 128 bytes.
const ENTRY_SECTORS: u64 = (ENTRIES * ENTRY_BYTES) as u64 / SECTOR as u64;
/// The partition starts at 1 MiB, the alignment every tool expects.
pub const FIRST_LBA: u64 = 2048;

/// The IEEE CRC-32 that GPT uses.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// A version-4-shaped GUID from `label`, in GPT's byte order.
pub fn guid(label: &str) -> [u8; 16] {
    let digest = Sha256::digest(label.as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out[7] = (out[7] & 0x0F) | 0x40;
    out[8] = (out[8] & 0x3F) | 0x80;
    out
}

/// Sectors a disk needs around a partition of `partition` sectors.
pub fn disk_sectors(partition: u64) -> u64 {
    // The backup array and header, then rounded up to whole MiB.
    (FIRST_LBA + partition + ENTRY_SECTORS + 1).next_multiple_of(2048)
}

fn protective_mbr(sectors: u64) -> [u8; 512] {
    let mut s = [0u8; 512];
    let entry = &mut s[446..462];
    entry[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    entry[4] = 0xEE;
    entry[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    entry[8..12].copy_from_slice(&1u32.to_le_bytes());
    let size = u32::try_from(sectors - 1).unwrap_or(u32::MAX);
    entry[12..16].copy_from_slice(&size.to_le_bytes());
    s[510] = 0x55;
    s[511] = 0xAA;
    s
}

fn entry_array(partition: u64, name: &str) -> Vec<u8> {
    let mut array = vec![0u8; (ENTRIES * ENTRY_BYTES) as usize];
    let entry = &mut array[..ENTRY_BYTES as usize];
    entry[..16].copy_from_slice(&ESP_TYPE);
    entry[16..32].copy_from_slice(&guid(&format!("{name} partition")));
    entry[32..40].copy_from_slice(&FIRST_LBA.to_le_bytes());
    entry[40..48].copy_from_slice(&(FIRST_LBA + partition - 1).to_le_bytes());
    for (i, unit) in "EFI system partition".encode_utf16().enumerate() {
        entry[56 + 2 * i..58 + 2 * i].copy_from_slice(&unit.to_le_bytes());
    }
    array
}

fn header(sectors: u64, primary: bool, entries_crc: u32, name: &str) -> [u8; 512] {
    let last = sectors - 1;
    let (mine, other, entries_at) = match primary {
        true => (1, last, 2),
        false => (last, 1, last - ENTRY_SECTORS),
    };
    let mut s = [0u8; 512];
    s[..8].copy_from_slice(b"EFI PART");
    s[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    s[12..16].copy_from_slice(&92u32.to_le_bytes());
    s[24..32].copy_from_slice(&mine.to_le_bytes());
    s[32..40].copy_from_slice(&other.to_le_bytes());
    s[40..48].copy_from_slice(&(2 + ENTRY_SECTORS).to_le_bytes());
    s[48..56].copy_from_slice(&(last - ENTRY_SECTORS - 1).to_le_bytes());
    s[56..72].copy_from_slice(&guid(&format!("{name} disk")));
    s[72..80].copy_from_slice(&entries_at.to_le_bytes());
    s[80..84].copy_from_slice(&ENTRIES.to_le_bytes());
    s[84..88].copy_from_slice(&ENTRY_BYTES.to_le_bytes());
    s[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = crc32(&s[..92]);
    s[16..20].copy_from_slice(&crc.to_le_bytes());
    s
}

/// A whole disk holding `volume` (whole sectors) as its EFI system partition.
/// `name` seeds the GUIDs.
pub fn disk(volume: &[u8], name: &str) -> Vec<u8> {
    let partition = volume.len() as u64 / SECTOR as u64;
    let sectors = disk_sectors(partition);
    let mut disk = vec![0u8; (sectors * SECTOR as u64) as usize];
    let at = |lba: u64| (lba * SECTOR as u64) as usize;
    let array = entry_array(partition, name);
    let crc = crc32(&array);
    disk[..512].copy_from_slice(&protective_mbr(sectors));
    disk[at(1)..at(2)].copy_from_slice(&header(sectors, true, crc, name));
    disk[at(2)..at(2) + array.len()].copy_from_slice(&array);
    disk[at(FIRST_LBA)..at(FIRST_LBA) + volume.len()].copy_from_slice(volume);
    let backup = sectors - 1 - ENTRY_SECTORS;
    disk[at(backup)..at(backup) + array.len()].copy_from_slice(&array);
    disk[at(sectors - 1)..].copy_from_slice(&header(sectors, false, crc, name));
    disk
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn guids_are_stable_and_version_4_shaped() {
        assert_eq!(guid("a"), guid("a"));
        assert_ne!(guid("a"), guid("b"));
        assert_eq!(guid("a")[7] >> 4, 4);
        assert_eq!(guid("a")[8] >> 6, 2);
    }

    #[test]
    fn a_disk_has_both_headers_and_a_valid_crc() {
        let volume = vec![0xAB; 4096 * 512];
        let disk = disk(&volume, "test");
        let sectors = disk.len() as u64 / 512;
        assert_eq!(sectors % 2048, 0);
        assert_eq!(&disk[510..512], &[0x55, 0xAA]);
        assert_eq!(disk[446 + 4], 0xEE);
        for lba in [1, sectors - 1] {
            let h = &disk[(lba * 512) as usize..(lba * 512 + 512) as usize];
            assert_eq!(&h[..8], b"EFI PART");
            let mut copy = h[..92].to_vec();
            let stored = u32::from_le_bytes(copy[16..20].try_into().unwrap());
            copy[16..20].fill(0);
            assert_eq!(crc32(&copy), stored);
            let mine = u64::from_le_bytes(h[24..32].try_into().unwrap());
            assert_eq!(mine, lba);
            let entries = u64::from_le_bytes(h[72..80].try_into().unwrap()) as usize * 512;
            let array = &disk[entries..entries + 16384];
            assert_eq!(&array[..16], &ESP_TYPE);
            assert_eq!(
                u32::from_le_bytes(h[88..92].try_into().unwrap()),
                crc32(array)
            );
        }
        let first = (FIRST_LBA * 512) as usize;
        assert!(disk[first..first + volume.len()].iter().all(|&b| b == 0xAB));
        // The partition ends where the entry says, before the backup array.
        let array = &disk[1024..1024 + 128];
        let last = u64::from_le_bytes(array[40..48].try_into().unwrap());
        assert_eq!(last, FIRST_LBA + 4095);
        assert!(last < sectors - 33);
    }
}
