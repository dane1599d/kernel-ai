//! A GPT disk with one partition, the EFI system partition: a protective MBR, the
//! primary header and entry array, the partition, and the backup array and header
//! at the end. GUIDs are derived from a label, so the same inputs give the same
//! disk: type GUIDs from the type's label, for good, and the disk's and each
//! partition's from the image's id and their role ([`unique`]), so two images
//! with different ids share no disk or partition GUID (ADR 0008 §1).

use super::SECTOR;

/// Sectors the entry array takes: 128 entries of 128 bytes.
const ENTRY_SECTORS: u64 = diskmap::ENTRY_SECTORS;
/// The partition starts at 1 MiB, the alignment every tool expects.
pub const FIRST_LBA: u64 = 2048;

/// A version-4-shaped GUID from `label`, and a disk's, a partition's or a
/// volume's own GUID from an image's id and what it is the GUID of (`disk`,
/// `partition kai-a`). The id is an image's seed (`release-image --image-id`);
/// each fixed default keeps CI's images byte-identical. The installer derives
/// an install's GUIDs with the same functions.
pub use installer::{guid, unique};

/// Sectors a disk needs around a partition of `partition` sectors.
pub fn disk_sectors(partition: u64) -> u64 {
    // The backup array and header, then rounded up to whole MiB.
    (FIRST_LBA + partition + ENTRY_SECTORS + 1).next_multiple_of(2048)
}

/// A whole disk holding `volume` (whole sectors) as its EFI system partition.
/// `image_id` seeds the disk's and the partition's GUIDs.
pub fn disk(volume: &[u8], image_id: &str) -> Vec<u8> {
    let partition = volume.len() as u64 / SECTOR as u64;
    let sectors = disk_sectors(partition);
    let esp = diskmap::Spec {
        kind: diskmap::ESP_TYPE,
        guid: diskmap::Guid(unique(image_id, "partition")),
        first: FIRST_LBA,
        last: FIRST_LBA + partition - 1,
        name: "EFI system partition",
    };
    let mut disk = laid_out(sectors, &unique(image_id, "disk"), &[esp]);
    let at = (FIRST_LBA * SECTOR as u64) as usize;
    disk[at..at + volume.len()].copy_from_slice(volume);
    disk
}

/// The GPT of `parts` on a disk of `sectors`, both in 512-byte sectors, as
/// a disk of `sector`-byte sectors holds it (G58): on a 4Kn disk the table's
/// LBAs are 4 KiB, the same partitions at the same bytes. The sectors to write
/// come back at 512-byte LBAs, so a writer seeks them as it always has. Every
/// partition must start and end on a whole disk sector.
pub fn layout_on(
    sector: usize,
    sectors: u64,
    disk_guid: diskmap::Guid,
    parts: &[diskmap::Spec<'_>],
) -> Result<Vec<(u64, Vec<u8>)>, String> {
    let per = (sector / SECTOR as usize) as u64;
    let whole = |lba: u64| lba.is_multiple_of(per);
    if !diskmap::sector_ok(sector)
        || !whole(sectors)
        || parts.iter().any(|p| !whole(p.first) || !whole(p.last + 1))
    {
        return Err(format!("the disk is not whole {sector}-byte sectors"));
    }
    let parts: Vec<_> = parts
        .iter()
        .map(|p| diskmap::Spec {
            first: p.first / per,
            last: (p.last + 1) / per - 1,
            ..p.clone()
        })
        .collect();
    let layout = diskmap::layout_in(sector, sectors / per, disk_guid, &parts)
        .map_err(|e| format!("the disk does not lay out: {e:?}"))?;
    Ok(layout.into_iter().map(|(lba, b)| (lba * per, b)).collect())
}

/// A zeroed disk of `sectors` with the GPT of `parts`.
pub fn laid_out(sectors: u64, disk_guid: &[u8; 16], parts: &[diskmap::Spec<'_>]) -> Vec<u8> {
    let mut disk = vec![0u8; (sectors * SECTOR as u64) as usize];
    let layout = diskmap::layout(sectors, diskmap::Guid(*disk_guid), parts)
        .expect("xtask lays out only disks that fit, with unique GUIDs");
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
    fn an_image_id_seeds_the_disk_and_partition_guids_but_not_the_type() {
        let volume = vec![0xAB; 4096 * 512];
        let (a, again, b) = (disk(&volume, "a"), disk(&volume, "a"), disk(&volume, "b"));
        assert_eq!(a, again);
        let disk_guid = |d: &[u8]| d[512 + 56..512 + 72].to_vec();
        let entry = |d: &[u8], at: usize| d[1024 + at..1024 + at + 16].to_vec();
        assert_ne!(disk_guid(&a), disk_guid(&b));
        assert_ne!(entry(&a, 16), entry(&b, 16));
        assert_eq!(entry(&a, 0), ESP_TYPE.0);
        assert_eq!(entry(&b, 0), ESP_TYPE.0);
        // The bootable image's fixed id is the label it always used.
        assert_eq!(unique("kernel-ai", "disk"), guid("kernel-ai disk"));
        assert_eq!(
            unique("kernel-ai", "partition"),
            guid("kernel-ai partition")
        );
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

    /// A 4Kn layout (G58) puts the same partitions at the same bytes, its
    /// LBAs in 4 KiB, and refuses a partition that is not whole 4 KiB sectors.
    #[test]
    fn a_4kn_layout_keeps_the_partitions_bytes() {
        let sectors = 64 * 2048;
        let parts = [diskmap::Spec {
            kind: ESP_TYPE,
            guid: diskmap::Guid(unique("x", "partition")),
            first: FIRST_LBA,
            last: FIRST_LBA + 16 * 2048 - 1,
            name: "EFI system partition",
        }];
        let guid = diskmap::Guid(unique("x", "disk"));
        let at_512 = layout_on(512, sectors, guid, &parts).unwrap();
        let at_4k = layout_on(4096, sectors, guid, &parts).unwrap();
        // Byte offsets: the header at 4096 on 4Kn, at 512 otherwise.
        assert_eq!(at_512[1].0 * 512, 512);
        assert_eq!(at_4k[1].0 * 512, 4096);
        let mut disk = vec![0u8; (sectors * 512) as usize];
        for (lba, bytes) in &at_4k {
            let at = (*lba * 512) as usize;
            disk[at..at + bytes.len()].copy_from_slice(bytes);
        }
        struct D(Vec<u8>);
        impl diskmap::Disk for D {
            fn sectors(&self) -> u64 {
                (self.0.len() / 4096) as u64
            }
            fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), diskmap::Error> {
                let at = lba as usize * 4096;
                out.copy_from_slice(&self.0[at..at + out.len()]);
                Ok(())
            }
            fn sector_size(&self) -> usize {
                4096
            }
        }
        let table = diskmap::read(&mut D(disk)).unwrap();
        let esp = table.find(ESP_TYPE).unwrap();
        assert_eq!(esp.first * 4096, FIRST_LBA * 512);
        assert_eq!(esp.sectors() * 4096, 16 * 2048 * 512);
        // A partition ending inside a 4 KiB sector.
        let mut odd = parts.clone();
        odd[0].last -= 1;
        assert!(layout_on(4096, sectors, guid, &odd).is_err());
        assert!(layout_on(1024, sectors, guid, &parts).is_err());
    }
}
