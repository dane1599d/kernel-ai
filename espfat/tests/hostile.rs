//! Hostile volumes: every bad boot sector, chain and directory is refused with an
//! error, never a panic, and a refused replace writes nothing.

mod common;

use common::*;
use espfat::{Error, SECTOR, Volume};

fn esp() -> Mem {
    image(&[
        Node::File("A.TXT", pattern(3000, 1)),
        Node::File("EMPTY", Vec::new()),
        Node::Dir(
            "EFI",
            vec![Node::Dir(
                "BOOT",
                vec![Node::File("BOOTX64.EFI", pattern(9000, 2))],
            )],
        ),
    ])
}

fn open(disk: &mut Mem) -> Result<Volume<&mut Mem>, Error> {
    let room = disk.size;
    Volume::open(disk, 0, room)
}

/// The first cluster of root file `n` (in [`esp`]'s order, 32 bytes an entry).
fn first_of(disk: &Mem, n: u64) -> u32 {
    let e = cluster_at(2) + 32 * n;
    let hi = disk.u32_at(e + 20) & 0xFFFF;
    let lo = disk.u32_at(e + 26) & 0xFFFF;
    (hi << 16) | lo
}

#[test]
fn the_image_reads_back() {
    let mut disk = esp();
    let mut v = open(&mut disk).unwrap();
    assert_eq!(v.read_file("A.TXT", usize::MAX).unwrap(), pattern(3000, 1));
    assert_eq!(
        v.read_file("/efi/boot/bootx64.efi", usize::MAX).unwrap(),
        pattern(9000, 2)
    );
    assert!(v.read_file("empty", usize::MAX).unwrap().is_empty());
    assert_eq!(v.geometry().clusters, CLUSTERS);
}

#[test]
fn bad_boot_sectors_are_refused() {
    let cases: &[(&str, usize, &[u8])] = &[
        ("signature", 510, &[0, 0]),
        ("4 KiB sectors", 11, &[0x00, 0x10]),
        ("3 sectors a cluster", 13, &[3]),
        ("0 sectors a cluster", 13, &[0]),
        ("no reserved sectors", 14, &[0, 0]),
        ("no FATs", 16, &[0]),
        ("three FATs", 16, &[3]),
        ("FAT16 root entries", 17, &[0, 2]),
        ("FAT16 sector count", 19, &[1, 0]),
        ("FAT16 FAT size", 22, &[1, 0]),
        ("larger than the disk", 32, &[0x71, 0x11, 0x01, 0x00]),
        ("too few clusters", 32, &[0x40, 0x9C, 0, 0]),
        ("an empty FAT", 36, &[0, 0, 0, 0]),
        ("a FAT too small", 36, &[0x00, 0x01, 0, 0]),
        ("FATs filling the volume", 36, &[0xFF, 0xFF, 0, 0]),
        ("an inactive mirror past the FATs", 40, &[0x82, 0]),
        ("a later FAT32 version", 42, &[1, 0]),
        ("root cluster 0", 44, &[0, 0, 0, 0]),
        ("root cluster past the end", 44, &[0xFF, 0xFF, 0xFF, 0x0F]),
    ];
    for (why, at, bytes) in cases {
        let mut disk = esp();
        disk.patch(*at as u64, bytes);
        match open(&mut disk) {
            Err(Error::NotFat32(_)) => {}
            Err(e) => panic!("{why}: {e:?}"),
            Ok(_) => panic!("{why}: mounted"),
        }
    }
    let mut disk = esp();
    let room = disk.size - 1;
    assert!(matches!(
        Volume::open(&mut disk, 0, room),
        Err(Error::NotFat32(_))
    ));
}

/// Opens `disk` and reads A.TXT, expecting the volume refused as corrupt.
fn corrupt(mut disk: Mem, why: &str) {
    let mut v = open(&mut disk).unwrap();
    match v.read_file("A.TXT", usize::MAX) {
        Err(Error::Corrupt(_)) => {}
        other => panic!("{why}: {:?}", other.map(|b| b.len())),
    }
    let mut logged = Logged {
        disk,
        log: Vec::new(),
    };
    let room = logged.disk.size;
    let mut v = Volume::open(&mut logged, 0, room).unwrap();
    assert!(
        matches!(v.replace("A.TXT", b"new"), Err(Error::Corrupt(_))),
        "{why}"
    );
    assert_eq!(logged.writes(), 0, "{why}: a refused replace wrote");
}

#[test]
fn bad_chains_are_refused() {
    let base = esp();
    let a = first_of(&base, 0);
    let set = |cluster: u32, value: u32| {
        let mut disk = base.clone();
        disk.patch(fat_at(0, cluster), &value.to_le_bytes());
        disk
    };
    corrupt(set(a + 2, a), "a loop back");
    corrupt(set(a + 1, a + 1), "a cluster naming itself");
    corrupt(set(a + 2, CLUSTERS + 2), "past the last cluster");
    corrupt(set(a + 2, 1), "cluster 1");
    corrupt(set(a + 2, 0), "a free cluster");
    corrupt(set(a + 2, 0x0FFF_FFF7), "a bad cluster");
    // A.TXT is six clusters, a to a + 5.
    corrupt(set(a + 5, a + 6), "longer than the size");
    corrupt(set(a + 5, 2), "longer, into the root");
    corrupt(set(a + 1, 0x0FFF_FFFF), "shorter than the size");
    corrupt(set(a + 4, 0x0FFF_FFF8), "shorter, at the other end mark");
    let mut disk = base.clone();
    disk.patch(cluster_at(2) + 26, &[0, 0]);
    disk.patch(cluster_at(2) + 20, &[0, 0]);
    corrupt(disk, "a file with no first cluster");
    let mut disk = base.clone();
    disk.patch(cluster_at(2) + 20, &[0xFF, 0x7F]);
    corrupt(disk, "a first cluster past the end");
}

#[test]
fn an_empty_file_with_clusters_is_refused() {
    let mut disk = esp();
    disk.patch(cluster_at(2) + 32 + 26, &[5, 0]);
    let mut v = open(&mut disk).unwrap();
    assert!(matches!(v.read_file("EMPTY", 1), Err(Error::Corrupt(_))));
}

#[test]
fn a_directory_that_loops_is_refused() {
    let mut disk = esp();
    // The root names itself and holds only deleted entries, so a search never
    // meets the end-of-directory mark.
    disk.patch(fat_at(0, 2), &2u32.to_le_bytes());
    for n in 0..16u64 {
        disk.patch(cluster_at(2) + 32 * n, &[0xE5]);
    }
    let mut v = open(&mut disk).unwrap();
    assert!(matches!(
        v.read_file("A.TXT", usize::MAX),
        Err(Error::Corrupt(_))
    ));
}

#[test]
fn bad_paths_and_limits() {
    let mut disk = esp();
    let mut v = open(&mut disk).unwrap();
    for path in ["", "/", "EFI//BOOT", "EFI/./BOOT", "../A.TXT", "EFI/"] {
        assert_eq!(
            v.read_file(path, usize::MAX),
            Err(Error::BadPath),
            "{path:?}"
        );
    }
    assert_eq!(v.read_file("B.TXT", usize::MAX), Err(Error::NotFound));
    assert_eq!(v.read_file("EFI/BOOT", usize::MAX), Err(Error::IsDir));
    assert_eq!(v.read_file("A.TXT/X", usize::MAX), Err(Error::NotDir));
    assert_eq!(v.read_file("A.TXT", 2999), Err(Error::TooLarge));
    assert_eq!(v.replace("EFI", b"x"), Err(Error::IsDir));
}

#[test]
fn a_replace_with_too_little_space_writes_nothing() {
    let disk = esp();
    let mut logged = Logged {
        disk,
        log: Vec::new(),
    };
    let room = logged.disk.size;
    let mut v = Volume::open(&mut logged, 0, room).unwrap();
    let big = vec![7u8; (CLUSTERS as usize) * SECTOR];
    assert_eq!(v.replace("A.TXT", &big), Err(Error::NoSpace));
    assert_eq!(logged.writes(), 0);
}

#[test]
fn a_failing_disk_is_an_error() {
    let mut disk = esp();
    disk.size = 1100;
    let mut v = Volume::open(&mut disk, 0, u64::from(SECTORS)).unwrap();
    assert_eq!(v.read_file("A.TXT", usize::MAX), Err(Error::Io));
}

/// Random bytes over the boot sector, the FATs' first sectors and the root:
/// whatever they say, mounting and reading return, and never panic.
#[test]
fn random_damage_never_panics() {
    let base = esp();
    let a = first_of(&base, 0);
    let regions = [
        (0u64, 90u64),
        (fat_at(0, 0), 64),
        (fat_at(0, a), 32),
        (fat_at(1, a), 32),
        (cluster_at(2), 128),
    ];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..4000 {
        let mut disk = base.clone();
        for _ in 0..1 + rng.next() % 4 {
            let (at, len) = regions[(rng.next() % regions.len() as u64) as usize];
            disk.patch(at + rng.next() % len, &[rng.next() as u8]);
        }
        let room = disk.size;
        if let Ok(mut v) = Volume::open(&mut disk, 0, room) {
            for path in ["A.TXT", "EMPTY", "EFI/BOOT/BOOTX64.EFI"] {
                let _ = v.read_file(path, 1 << 20);
            }
            let _ = v.replace("A.TXT", &pattern(700, 9));
        }
    }
}
