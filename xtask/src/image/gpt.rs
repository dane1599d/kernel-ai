//! A GPT disk with one partition, the EFI system partition: a protective MBR, the
//! primary header and entry array, the partition, and the backup array and header
//! at the end. GUIDs are derived from a label, so the same inputs give the same
//! disk.

use super::SECTOR;
use sha2::{Digest, Sha256};

/// Sectors the entry array takes: 128 entries of 128 bytes.
const ENTRY_SECTORS: u64 = diskmap::ENTRY_SECTORS;
/// The partition starts at 1 MiB, the alignment every tool expects.
pub const FIRST_LBA: u64 = 2048;

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

/// A whole disk holding `volume` (whole sectors) as its EFI system partition.
/// `name` seeds the GUIDs.
pub fn disk(volume: &[u8], name: &str) -> Vec<u8> {
    let partition = volume.len() as u64 / SECTOR as u64;
    let sectors = disk_sectors(partition);
    let esp = diskmap::Spec {
        kind: diskmap::ESP_TYPE,
        guid: diskmap::Guid(guid(&format!("{name} partition"))),
        first: FIRST_LBA,
        last: FIRST_LBA + partition - 1,
        name: "EFI system partition",
    };
    let mut disk = laid_out(sectors, &guid(&format!("{name} disk")), &[esp]);
    let at = (FIRST_LBA * SECTOR as u64) as usize;
    disk[at..at + volume.len()].copy_from_slice(volume);
    disk
}

/// A zeroed disk of `sectors` with the GPT of `parts`.
pub fn laid_out(sectors: u64, disk_guid: &[u8; 16], parts: &[diskmap::Spec<'_>]) -> Vec<u8> {
    let mut disk = vec![0u8; (sectors * SECTOR as u64) as usize];
    let layout = diskmap::layout(sectors, diskmap::Guid(*disk_guid), parts)
        .expect("xtask lays out only disks that fit");
    for (lba, bytes) in layout {
        let at = (lba * SECTOR as u64) as usize;
        disk[at..at + bytes.len()].copy_from_slice(&bytes);
    }
    disk
}

#[cfg(test)]
mod tests {
    use super::*;
    use diskmap::{ESP_TYPE, crc32};

    #[test]
    fn diskmaps_type_guids_are_the_labels_ones() {
        assert_eq!(diskmap::KAI_DATA_TYPE.0, guid("kai-data partition type"));
        assert_eq!(diskmap::KAI_CACHE_TYPE.0, guid("kai-cache partition type"));
        assert_eq!(diskmap::KAI_R_TYPE.0, guid("kai-r partition type"));
        assert_eq!(diskmap::KAI_A_TYPE.0, guid("kai-a partition type"));
        assert_eq!(diskmap::KAI_B_TYPE.0, guid("kai-b partition type"));
    }

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
            assert_eq!(&array[..16], &ESP_TYPE.0);
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
