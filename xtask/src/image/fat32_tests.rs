//! The FAT32 writer, checked by reading volumes back with a reader written here
//! from the specification: boot sector fields, FAT chains, long names and their
//! checksums.

use super::*;

/// A minimal FAT32 reader over a whole volume.
struct Reader<'a> {
    volume: &'a [u8],
    spc: usize,
    fat_at: usize,
    data_at: usize,
    root: u32,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

impl<'a> Reader<'a> {
    fn new(volume: &'a [u8]) -> Self {
        assert_eq!(u16_at(volume, 11), 512);
        assert_eq!(&volume[82..90], b"FAT32   ");
        assert_eq!(&volume[510..512], &[0x55, 0xAA]);
        assert_eq!(
            &volume[..512],
            &volume[6 * 512..7 * 512],
            "backup boot sector"
        );
        let spc = volume[13] as usize;
        let reserved = u16_at(volume, 14) as usize;
        let fats = volume[16] as usize;
        let fat_sectors = u32_at(volume, 36) as usize;
        let fat_at = reserved * 512;
        let fat_len = fat_sectors * 512;
        assert_eq!(
            &volume[fat_at..fat_at + fat_len],
            &volume[fat_at + fat_len..fat_at + 2 * fat_len],
            "the two FATs match"
        );
        Self {
            volume,
            spc,
            fat_at,
            data_at: (reserved + fats * fat_sectors) * 512,
            root: u32_at(volume, 44),
        }
    }

    fn chain(&self, first: u32, size: Option<usize>) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cluster = first;
        let bytes = self.spc * 512;
        while (2..0x0FFF_FFF8).contains(&cluster) {
            let at = self.data_at + (cluster as usize - 2) * bytes;
            out.extend_from_slice(&self.volume[at..at + bytes]);
            cluster = u32_at(self.volume, self.fat_at + 4 * cluster as usize) & 0x0FFF_FFFF;
        }
        if let Some(size) = size {
            assert!(
                out.len() >= size && out.len() - size < bytes,
                "chain length"
            );
            out.truncate(size);
        }
        out
    }

    /// (name, attr, first cluster, size) of each entry in a directory.
    fn list(&self, cluster: u32) -> Vec<(String, u8, u32, usize)> {
        let bytes = self.chain(cluster, None);
        let mut out = Vec::new();
        let mut long: Vec<(u8, Vec<u16>, u8)> = Vec::new();
        for entry in bytes.chunks(32) {
            match entry[0] {
                0 => break,
                0xE5 => continue,
                _ => {}
            }
            if entry[11] == 0x0F {
                let mut units = Vec::new();
                for at in (1..11)
                    .step_by(2)
                    .chain((14..26).step_by(2))
                    .chain((28..32).step_by(2))
                {
                    units.push(u16_at(entry, at));
                }
                long.push((entry[0], units, entry[13]));
                continue;
            }
            let short: [u8; 11] = entry[..11].try_into().unwrap();
            let mut sum: u8 = 0;
            for b in short {
                sum = sum.rotate_right(1).wrapping_add(b);
            }
            let name = if long.is_empty() {
                let base = String::from_utf8_lossy(&short[..8]).trim_end().to_string();
                let ext = String::from_utf8_lossy(&short[8..]).trim_end().to_string();
                if ext.is_empty() {
                    base
                } else {
                    format!("{base}.{ext}")
                }
            } else {
                assert_eq!(
                    long[0].0 & 0x40,
                    0x40,
                    "the first long entry is the last part"
                );
                let mut units = Vec::new();
                for (order, part, check) in long.iter().rev() {
                    assert_eq!(*check, sum, "long-name checksum");
                    assert_eq!(order & 0x3F, (units.len() / 13 + 1) as u8);
                    units.extend_from_slice(part);
                }
                let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
                assert!(units[end..].iter().skip(1).all(|&u| u == 0xFFFF));
                String::from_utf16(&units[..end]).unwrap()
            };
            long.clear();
            let first = (u32::from(u16_at(entry, 20)) << 16) | u32::from(u16_at(entry, 26));
            out.push((name, entry[11], first, u32_at(entry, 28) as usize));
        }
        out
    }

    fn open(&self, path: &str) -> Vec<u8> {
        let mut cluster = self.root;
        let parts: Vec<&str> = path.split('/').collect();
        for (i, part) in parts.iter().enumerate() {
            let entries = self.list(cluster);
            let (_, attr, first, size) = entries
                .iter()
                .find(|(name, ..)| name.eq_ignore_ascii_case(part))
                .unwrap_or_else(|| panic!("{part} of {path} not found"));
            if i + 1 == parts.len() {
                assert_eq!(attr & ATTR_DIR, 0);
                return self.chain(*first, Some(*size));
            }
            assert_eq!(attr & ATTR_DIR, ATTR_DIR);
            cluster = *first;
        }
        unreachable!()
    }
}

fn file(name: &str, bytes: Vec<u8>) -> Node {
    Node::File {
        name: name.into(),
        bytes,
    }
}

fn dir(name: &str, children: Vec<Node>) -> Node {
    Node::Dir {
        name: name.into(),
        children,
    }
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn esp_like() -> Vec<Node> {
    let users = (0..24)
        .flat_map(|i| {
            [
                file(
                    &format!("program-{i}.elf"),
                    pattern(3000 + i * 700, i as u8),
                ),
                file(&format!("program-{i}.lines"), pattern(100 + i, 7)),
            ]
        })
        .collect();
    vec![
        dir(
            "EFI",
            vec![dir("BOOT", vec![file("BOOTX64.EFI", pattern(70_000, 1))])],
        ),
        dir(
            "boot",
            vec![
                file("kernel", pattern(1_500_000, 2)),
                file("kernel.lines", pattern(9_999, 3)),
                file("empty", Vec::new()),
                dir(
                    "limine",
                    vec![file("limine.conf", b"timeout: 0\n".to_vec())],
                ),
                dir("user", users),
            ],
        ),
    ]
}

#[test]
fn geometry_follows_the_fat32_table() {
    assert!(Geometry::for_sectors(66_600).is_err());
    let small = Geometry::for_sectors(131_072).unwrap();
    assert_eq!(small.sectors_per_cluster, 1);
    assert!(small.clusters() >= 65_525);
    // The FAT covers every cluster.
    assert!(small.fat_sectors * 128 >= small.clusters() + 2);
    let large = Geometry::for_sectors(600_000).unwrap();
    assert_eq!(large.sectors_per_cluster, 8);
    assert!(large.clusters() >= 65_525);
}

#[test]
fn short_names_and_aliases() {
    assert!(is_short("BOOTX64.EFI"));
    assert!(is_short("EFI"));
    assert!(!is_short("boot"));
    assert!(!is_short("kernel.lines"));
    assert_eq!(&short_name("BOOTX64.EFI", 0), b"BOOTX64 EFI");
    assert_eq!(&short_name("kernel.lines", 1), b"KERNEL~1LIN");
    assert_eq!(&short_name("notes-v1.wasm", 12), b"NOTES~12WAS");
    assert_eq!(&short_name("boot", 2), b"BOOT~2     ");
}

#[test]
fn every_file_reads_back_byte_for_byte() {
    let nodes = esp_like();
    let volume = volume(131_072, 2048, 0x1234, &nodes).unwrap();
    let reader = Reader::new(&volume);
    assert_eq!(reader.open("EFI/BOOT/BOOTX64.EFI"), pattern(70_000, 1));
    assert_eq!(reader.open("boot/kernel"), pattern(1_500_000, 2));
    assert_eq!(reader.open("boot/kernel.lines"), pattern(9_999, 3));
    assert!(reader.open("boot/empty").is_empty());
    assert_eq!(reader.open("boot/limine/limine.conf"), b"timeout: 0\n");
    for i in 0..24 {
        let elf = reader.open(&format!("boot/user/program-{i}.elf"));
        assert_eq!(elf, pattern(3000 + i * 700, i as u8));
    }
    // Every directory below the root starts with "." and "..", and ".." of a
    // directory in the root names cluster 0.
    let boot = reader.list(reader.root);
    let (_, _, boot_cluster, _) = boot.iter().find(|(n, ..)| n == "boot").unwrap();
    let inside = reader.list(*boot_cluster);
    assert_eq!((inside[0].0.as_str(), inside[0].2), (".", *boot_cluster));
    assert_eq!((inside[1].0.as_str(), inside[1].2), ("..", 0));
    let (_, _, user, _) = inside.iter().find(|(n, ..)| n == "user").unwrap();
    assert_eq!(reader.list(*user)[1].2, *boot_cluster);
    // The hidden-sector count is the partition's start.
    assert_eq!(u32_at(&volume, 28), 2048);
}

#[test]
fn the_same_files_give_the_same_volume() {
    let a = volume(131_072, 2048, 1, &esp_like()).unwrap();
    let b = volume(131_072, 2048, 1, &esp_like()).unwrap();
    assert!(a == b);
}

#[test]
fn files_that_do_not_fit_are_refused() {
    let big = vec![file("big", vec![0; 70 * 1024 * 1024])];
    assert!(volume(131_072, 2048, 1, &big).is_err());
}
