//! `replace`, cut at every sector write and at random subsets of each flush's
//! writes. Every cut leaves the old file or the new one, read the same through
//! both FAT copies, and every other file untouched.

mod common;

use common::*;
use espfat::{SECTOR, Volume};

const OLD: usize = 70_000;
const NEW: usize = 90_000;

fn esp() -> Mem {
    image(&[
        Node::File("A.TXT", pattern(3000, 1)),
        Node::File("BIG.BIN", pattern(OLD, 2)),
        Node::File("EMPTY", Vec::new()),
        Node::Dir(
            "EFI",
            vec![Node::Dir(
                "BOOT",
                vec![Node::File("BOOTX64.EFI", pattern(9000, 3))],
            )],
        ),
    ])
}

fn read(disk: &mut Mem, copy: u32, path: &str) -> Vec<u8> {
    let room = disk.size;
    let mut v = Volume::open(disk, 0, room).unwrap();
    v.read_through(copy).unwrap();
    v.read_file(path, usize::MAX)
        .unwrap_or_else(|e| panic!("{path} through FAT {copy}: {e:?}"))
}

/// Which of `old` and `new` the file at `path` holds, through both FAT copies,
/// with every other file as [`esp`] made it.
fn state(disk: &mut Mem, path: &str, old: &[u8], new: &[u8]) -> bool {
    let mut seen = Vec::new();
    for copy in 0..2 {
        let got = read(disk, copy, path);
        assert!(got == old || got == new, "FAT {copy} reads neither file");
        seen.push(got == new);
        for (other, bytes) in [
            ("A.TXT", pattern(3000, 1)),
            ("EFI/BOOT/BOOTX64.EFI", pattern(9000, 3)),
            ("BIG.BIN", pattern(OLD, 2)),
        ] {
            if other != path {
                assert_eq!(read(disk, copy, other), bytes, "{other} through FAT {copy}");
            }
        }
    }
    assert_eq!(seen[0], seen[1], "the FAT copies read different files");
    seen[0]
}

/// Replaces `path` on a copy of `base`, logging every write.
fn logged_replace(base: &Mem, path: &str, bytes: &[u8]) -> Logged {
    let mut logged = Logged {
        disk: base.clone(),
        log: Vec::new(),
    };
    let room = logged.disk.size;
    Volume::open(&mut logged, 0, room)
        .unwrap()
        .replace(path, bytes)
        .unwrap();
    logged
}

/// Every FAT entry's cluster that is not free, in FAT copy `copy`.
fn used(disk: &Mem, copy: u32) -> usize {
    (2..CLUSTERS + 2)
        .filter(|&c| disk.u32_at(fat_at(copy, c)) & 0x0FFF_FFFF != 0)
        .count()
}

/// The finished volume: both FATs identical, no leaked cluster, and FSInfo's
/// count right.
fn settled(disk: &Mem, files: usize) {
    for s in 0..FAT_SECTORS {
        let a = disk.get(u64::from(RESERVED + s));
        let b = disk.get(u64::from(RESERVED + FAT_SECTORS + s));
        assert!(a == b, "FAT sector {s} differs between the copies");
    }
    // The root, EFI and BOOT take a cluster each.
    let clusters = 3 + files.div_ceil(SECTOR);
    assert_eq!(used(disk, 0), clusters, "leaked or missing clusters");
    let free = disk.u32_at(SECTOR as u64 + 488);
    assert_eq!(free as usize, CLUSTERS as usize - clusters);
}

fn total(sizes: &[usize]) -> usize {
    // Each file's own whole clusters.
    sizes.iter().map(|s| s.div_ceil(SECTOR) * SECTOR).sum()
}

#[test]
fn a_whole_replace_reads_back_and_settles() {
    let new = pattern(NEW, 4);
    let mut done = logged_replace(&esp(), "BIG.BIN", &new).disk;
    assert!(state(&mut done, "BIG.BIN", &pattern(OLD, 2), &new));
    settled(&done, total(&[3000, NEW, 9000]));
}

#[test]
fn a_cut_after_any_write_leaves_the_old_file_or_the_new() {
    let base = esp();
    let old = pattern(OLD, 2);
    let new = pattern(NEW, 4);
    let logged = logged_replace(&base, "BIG.BIN", &new);
    let n = logged.writes();
    // 176 data sectors, at least two FAT sectors per copy, the entry, FSInfo.
    assert!(n > 180, "{n} writes");
    let mut flipped = None;
    for k in 0..=n {
        let mut disk = cut_at(&base, &logged.log, k);
        let is_new = state(&mut disk, "BIG.BIN", &old, &new);
        match (is_new, flipped) {
            (true, None) => flipped = Some(k),
            (false, Some(at)) => panic!("old again at {k} after new at {at}"),
            _ => {}
        }
        // Running the update again after the cut finishes it.
        let room = disk.size;
        Volume::open(&mut disk, 0, room)
            .unwrap()
            .replace("BIG.BIN", &new)
            .unwrap();
        assert!(
            state(&mut disk, "BIG.BIN", &old, &new),
            "the rerun after {k}"
        );
    }
    assert!(
        flipped.is_some_and(|k| k < n),
        "never switched before the last write"
    );
}

#[test]
fn a_cut_keeping_any_subset_of_one_flushes_writes_is_safe() {
    let base = esp();
    let old = pattern(OLD, 2);
    let new = pattern(NEW, 4);
    let epochs = logged_replace(&base, "BIG.BIN", &new).epochs();
    assert_eq!(epochs.len(), 5, "five phases");
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for (i, epoch) in epochs.iter().enumerate() {
        for _ in 0..24 {
            let mut disk = base.clone();
            for (lba, s) in epochs[..i].iter().flatten() {
                disk.sectors.insert(*lba, s.clone());
            }
            for (lba, s) in epoch {
                if rng.next().is_multiple_of(2) {
                    disk.sectors.insert(*lba, s.clone());
                }
            }
            let is_new = state(&mut disk, "BIG.BIN", &old, &new);
            // Phase 3 is the switch: its one write decides.
            if i != 2 {
                assert_eq!(is_new, i > 2, "phase {} decided the file", i + 1);
            }
            // The volume checks, and a reclaim settles it without changing
            // which file it holds.
            let room = disk.size;
            let mut v = Volume::open(&mut disk, 0, room).unwrap();
            let usage = v.check().unwrap();
            assert_eq!(v.reclaim().unwrap(), usage.leaked);
            assert_eq!(state(&mut disk, "BIG.BIN", &old, &new), is_new);
            settled(&disk, total(&[3000, if is_new { NEW } else { OLD }, 9000]));
        }
    }
}

#[test]
fn files_grow_shrink_and_empty_in_place() {
    let mut disk = esp();
    let room = disk.size;
    let sizes = [0, 1, SECTOR, SECTOR + 1, 200_000, 5, 0, 70_000];
    for (round, &size) in sizes.iter().enumerate() {
        let bytes = pattern(size, round as u8);
        let mut v = Volume::open(&mut disk, 0, room).unwrap();
        v.replace("EFI/BOOT/BOOTX64.EFI", &bytes).unwrap();
        v.replace("EMPTY", &bytes).unwrap();
        for copy in 0..2 {
            assert_eq!(read(&mut disk, copy, "efi/boot/bootx64.efi"), bytes);
            assert_eq!(read(&mut disk, copy, "EMPTY"), bytes);
            assert_eq!(read(&mut disk, copy, "BIG.BIN"), pattern(OLD, 2));
        }
        settled(&disk, total(&[3000, OLD, size, size]));
    }
}

/// Clusters of four sectors: a file's last cluster is part full, and each cut
/// still leaves the old file or the new.
#[test]
fn four_sector_clusters_cut_anywhere() {
    let layout = Layout::new(4, 270_000);
    let base = image_with(
        layout,
        &[
            Node::File("A.TXT", pattern(5000, 1)),
            Node::File("BIG.BIN", pattern(20_000, 2)),
        ],
    );
    let room = base.size;
    let (old, new) = (pattern(20_000, 2), pattern(30_001, 4));
    let logged = logged_replace(&base, "BIG.BIN", &new);
    let mut switched = false;
    for k in 0..=logged.writes() {
        let mut disk = cut_at(&base, &logged.log, k);
        let mut seen = Vec::new();
        for copy in 0..2 {
            let mut v = Volume::open(&mut disk, 0, room).unwrap();
            assert_eq!(v.geometry().sectors_per_cluster, 4);
            v.read_through(copy).unwrap();
            let got = v.read_file("BIG.BIN", usize::MAX).unwrap();
            assert!(got == old || got == new, "after write {k}, FAT {copy}");
            seen.push(got == new);
            assert_eq!(v.read_file("A.TXT", usize::MAX).unwrap(), pattern(5000, 1));
        }
        assert_eq!(seen[0], seen[1]);
        assert!(!switched || seen[0]);
        switched = seen[0];
    }
    assert!(switched);
}
