//! The whole-volume check: chains that two files share, FAT copies that disagree
//! and directories that loop are refused before a replace writes anything.
//! Clusters a cut leaks are counted and reclaimed, safely even if the reclaim
//! is itself cut.

mod common;

use common::*;
use espfat::{Error, SECTOR, Usage, Volume};

const OLD: usize = 70_000;
const NEW: usize = 90_000;

fn esp() -> Mem {
    image(&[
        Node::File("A.TXT", pattern(3000, 1)),
        Node::File("BIG.BIN", pattern(OLD, 2)),
        Node::Dir(
            "EFI",
            vec![Node::Dir(
                "BOOT",
                vec![Node::File("BOOTX64.EFI", pattern(9000, 3))],
            )],
        ),
    ])
}

/// Clusters the files and directories of [`esp`] take with `big` bytes in
/// BIG.BIN: the root, EFI and BOOT one each.
fn named(big: usize) -> u32 {
    (3 + [3000, big, 9000]
        .iter()
        .map(|s| s.div_ceil(SECTOR))
        .sum::<usize>()) as u32
}

fn check(disk: &mut Mem) -> Result<Usage, Error> {
    let room = disk.size;
    Volume::open(disk, 0, room)?.check()
}

/// Root entry `n`'s first cluster (32 bytes an entry).
fn first_of(disk: &Mem, n: u64) -> u32 {
    let e = cluster_at(2) + 32 * n;
    ((disk.u32_at(e + 20) & 0xFFFF) << 16) | (disk.u32_at(e + 26) & 0xFFFF)
}

/// Expects `disk` refused as corrupt by the check, and by a replace that then
/// writes nothing.
fn refused(disk: Mem, why: &str) {
    let mut logged = Logged {
        disk,
        log: Vec::new(),
    };
    let room = logged.disk.size;
    let mut v = Volume::open(&mut logged, 0, room).unwrap();
    assert!(matches!(v.check(), Err(Error::Corrupt(_))), "{why}: check");
    assert!(
        matches!(v.replace("A.TXT", b"x"), Err(Error::Corrupt(_))),
        "{why}: replace"
    );
    assert_eq!(logged.writes(), 0, "{why}: a refused replace wrote");
}

/// [`refused`], with the check's error exactly `err`.
fn refused_as(mut disk: Mem, err: &'static str) {
    assert_eq!(check(&mut disk), Err(Error::Corrupt(err)));
    refused(disk, err);
}

/// `path` read through FAT copy `copy`.
fn read(disk: &mut Mem, copy: u32, path: &str) -> Vec<u8> {
    let room = disk.size;
    let mut v = Volume::open(disk, 0, room).unwrap();
    v.read_through(copy).unwrap();
    v.read_file(path, usize::MAX).unwrap()
}

/// A last (and only) long-name entry holding `name`, for the 8.3 name `short`.
fn lfn(name: &str, short: &[u8; 11]) -> [u8; 32] {
    const AT: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    let units: Vec<u16> = name.encode_utf16().collect();
    let mut e = [0u8; 32];
    e[0] = 0x41;
    e[11] = 0x0F;
    e[13] = short
        .iter()
        .fold(0u8, |sum, &b| sum.rotate_right(1).wrapping_add(b));
    for (i, &at) in AT.iter().enumerate() {
        let u = match i.cmp(&units.len()) {
            std::cmp::Ordering::Less => units[i],
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 0xFFFF,
        };
        e[at..at + 2].copy_from_slice(&u.to_le_bytes());
    }
    e
}

#[test]
fn a_sound_volume_checks_with_nothing_leaked() {
    let mut disk = esp();
    assert_eq!(
        check(&mut disk),
        Ok(Usage {
            named: named(OLD),
            leaked: 0,
            bad: 0
        })
    );
}

#[test]
fn a_cluster_two_files_name_is_refused() {
    let mut disk = esp();
    // BIG.BIN's entry names A.TXT's first cluster, and its size A.TXT's.
    let a = first_of(&disk, 0);
    disk.patch(cluster_at(2) + 32 + 26, &(a as u16).to_le_bytes());
    disk.patch(cluster_at(2) + 32 + 28, &3000u32.to_le_bytes());
    refused(disk, "a shared chain");
}

#[test]
fn fat_copies_that_disagree_on_a_chain_are_refused() {
    // BIG.BIN's chain, in the second copy only, skips a cluster that stays
    // marked used there.
    let mut disk = esp();
    let big = first_of(&disk, 1);
    disk.patch(fat_at(1, big + 3), &(big + 5).to_le_bytes());
    disk.patch(fat_at(1, big + 136), &0x0FFF_FFFFu32.to_le_bytes());
    refused(disk, "a chain only one copy has");
    // A file's last cluster ends the chain in one copy and goes on in the other.
    let mut disk = esp();
    let a = first_of(&disk, 0);
    disk.patch(fat_at(1, a + 5), &(a + 6).to_le_bytes());
    refused(disk, "a longer chain in the second copy");
}

/// Chains that each copy holds soundly, of the same length, but through other
/// clusters: only comparing the copies finds them.
#[test]
fn sound_chains_that_differ_between_the_copies_are_refused() {
    const WHY: &str = "the FAT copies disagree on a chain";
    let spare = CLUSTERS; // Free in both copies.
    // BIG.BIN skips a cluster in the second copy and ends one further on.
    let mut disk = esp();
    let big = first_of(&disk, 1);
    disk.patch(fat_at(1, big + 3), &(big + 5).to_le_bytes());
    disk.patch(fat_at(1, big + 136), &spare.to_le_bytes());
    disk.patch(fat_at(1, spare), &0x0FFF_FFFFu32.to_le_bytes());
    refused_as(disk, WHY);
    // The root has a second cluster in the second copy only.
    let mut disk = esp();
    disk.patch(fat_at(1, 2), &spare.to_le_bytes());
    disk.patch(fat_at(1, spare), &0x0FFF_FFFFu32.to_le_bytes());
    refused_as(disk, WHY);
}

#[test]
fn a_directory_named_twice_is_refused() {
    // A second root entry for EFI.
    let mut disk = esp();
    let efi = disk.get(u64::from(DATA)).to_vec();
    disk.patch(cluster_at(2) + 32 * 3, &efi[64..96]);
    refused_as(disk, "two chains share a cluster");
}

/// A "." entry is skipped by the walk, so no lookup may find it either, even
/// with a long name before it and a file's attributes.
#[test]
fn a_dot_entry_is_never_found_by_a_long_name() {
    let mut disk = esp();
    let (efi, a) = (first_of(&disk, 2), first_of(&disk, 0));
    let entries = disk.get(u64::from(DATA + efi - 2)).to_vec();
    // [lfn "DOT.TXT", "." as a file over A.TXT's chain, "..", BOOT]
    let mut dot = entries[..32].to_vec();
    dot[11] = 0x20;
    dot[26..28].copy_from_slice(&(a as u16).to_le_bytes());
    dot[28..32].copy_from_slice(&3000u32.to_le_bytes());
    let mut sector = lfn("DOT.TXT", b".          ").to_vec();
    sector.extend(&dot);
    sector.extend(&entries[32..96]);
    disk.patch(cluster_at(efi), &sector);
    let mut logged = Logged {
        disk,
        log: Vec::new(),
    };
    let room = logged.disk.size;
    let mut v = Volume::open(&mut logged, 0, room).unwrap();
    assert_eq!(v.read_file("EFI/DOT.TXT", usize::MAX), Err(Error::NotFound));
    assert_eq!(v.replace("EFI/DOT.TXT", b"x"), Err(Error::NotFound));
    assert_eq!(
        v.read_file("EFI/BOOT/BOOTX64.EFI", usize::MAX).unwrap(),
        pattern(9000, 3)
    );
    assert_eq!(logged.writes(), 0);
}

/// A bad mark, in either copy, is never reclaimed and never counted free.
#[test]
fn bad_clusters_stay_marked() {
    const BAD: u32 = 0x0FFF_FFF7;
    let mut disk = esp();
    let (one, both, leak) = (CLUSTERS - 2, CLUSTERS - 1, CLUSTERS);
    disk.patch(fat_at(0, one), &BAD.to_le_bytes());
    disk.patch(fat_at(0, both), &BAD.to_le_bytes());
    disk.patch(fat_at(1, both), &BAD.to_le_bytes());
    disk.patch(fat_at(1, leak), &0x0FFF_FFFFu32.to_le_bytes());
    let usage = Usage {
        named: named(OLD),
        leaked: 1,
        bad: 2,
    };
    assert_eq!(check(&mut disk), Ok(usage));
    let room = disk.size;
    assert_eq!(Volume::open(&mut disk, 0, room).unwrap().reclaim(), Ok(1));
    assert_eq!(check(&mut disk), Ok(Usage { leaked: 0, ..usage }));
    assert_eq!(disk.u32_at(fat_at(0, one)), BAD);
    assert_eq!(disk.u32_at(fat_at(1, one)), 0);
    assert_eq!(disk.u32_at(fat_at(0, both)), BAD);
    assert_eq!(disk.u32_at(fat_at(1, both)), BAD);
    assert_eq!(disk.u32_at(fat_at(1, leak)), 0);
    assert_eq!(disk.u32_at(SECTOR as u64 + 488), CLUSTERS - named(OLD) - 2);
}

#[test]
fn a_directory_naming_its_parent_is_refused() {
    let mut disk = esp();
    // EFI/BOOT's entry names the root's cluster.
    let efi = first_of(&disk, 2);
    disk.patch(cluster_at(efi) + 64 + 26, &2u16.to_le_bytes());
    refused(disk, "a directory loop");
}

#[test]
fn a_size_larger_than_the_volume_is_refused_before_its_chain_is_walked() {
    let mut disk = esp();
    let a = first_of(&disk, 0);
    disk.patch(fat_at(0, a + 5), &a.to_le_bytes());
    disk.patch(cluster_at(2) + 28, &u32::MAX.to_le_bytes());
    let room = disk.size;
    let mut v = Volume::open(&mut disk, 0, room).unwrap();
    let err = v.read_file("A.TXT", usize::MAX).unwrap_err();
    assert_eq!(err, Error::Corrupt("a file is larger than the volume"));
}

/// After a cut at any write of a replace, the check counts what leaked, and a
/// reclaim, itself cut at any of its writes, frees it all and loses nothing.
#[test]
fn a_reclaim_after_any_cut_frees_every_leak() {
    let base = esp();
    let (old, new) = (pattern(OLD, 2), pattern(NEW, 4));
    let mut logged = Logged {
        disk: base.clone(),
        log: Vec::new(),
    };
    let room = base.size;
    Volume::open(&mut logged, 0, room)
        .unwrap()
        .replace("BIG.BIN", &new)
        .unwrap();
    let mut leaks = 0;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for k in 0..=logged.writes() {
        let cut = cut_at(&base, &logged.log, k);
        let mut disk = cut.clone();
        let usage = check(&mut disk).unwrap();
        let room = disk.size;
        let big = Volume::open(&mut disk, 0, room)
            .unwrap()
            .read_file("BIG.BIN", usize::MAX)
            .unwrap();
        let size = if big == new { NEW } else { OLD };
        assert!(big == old || big == new);
        assert_eq!(usage.named, named(size), "after write {k}");
        leaks += usage.leaked;

        let mut reclaiming = Logged {
            disk: cut.clone(),
            log: Vec::new(),
        };
        let freed = Volume::open(&mut reclaiming, 0, room)
            .unwrap()
            .reclaim()
            .unwrap();
        assert_eq!(freed, usage.leaked);
        // Cut at every write, and keep random subsets of each flush's writes
        // on top of the earlier flushes; every file reads the same through
        // both copies, and a second reclaim finishes.
        let mut cuts: Vec<Mem> = (0..=reclaiming.writes())
            .map(|j| cut_at(&cut, &reclaiming.log, j))
            .collect();
        let epochs = reclaiming.epochs();
        for (i, epoch) in epochs.iter().enumerate() {
            for _ in 0..4 {
                let mut disk = cut.clone();
                for (lba, s) in epochs[..i].iter().flatten() {
                    disk.sectors.insert(*lba, s.clone());
                }
                for (lba, s) in epoch {
                    if rng.next().is_multiple_of(2) {
                        disk.sectors.insert(*lba, s.clone());
                    }
                }
                cuts.push(disk);
            }
        }
        for mut disk in cuts {
            for copy in 0..2 {
                assert_eq!(read(&mut disk, copy, "BIG.BIN"), big);
                assert_eq!(read(&mut disk, copy, "A.TXT"), pattern(3000, 1));
            }
            let room = disk.size;
            Volume::open(&mut disk, 0, room).unwrap().reclaim().unwrap();
            assert_eq!(check(&mut disk).unwrap().leaked, 0, "after write {k}");
        }
        let mut done = reclaiming.disk;
        assert_eq!(
            check(&mut done),
            Ok(Usage {
                named: named(size),
                leaked: 0,
                bad: 0
            })
        );
        for s in 0..FAT_SECTORS {
            let (a, b) = (
                done.get(u64::from(RESERVED + s)),
                done.get(u64::from(RESERVED + FAT_SECTORS + s)),
            );
            assert!(
                a == b,
                "after write {k}, FAT sector {s} differs between the copies"
            );
        }
        assert_eq!(done.u32_at(SECTOR as u64 + 488), CLUSTERS - named(size));
    }
    assert!(leaks > 0, "no cut leaked anything, so nothing was tested");
}
