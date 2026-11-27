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

#[test]
fn a_sound_volume_checks_with_nothing_leaked() {
    let mut disk = esp();
    assert_eq!(
        check(&mut disk),
        Ok(Usage {
            named: named(OLD),
            leaked: 0
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
        for j in 0..=reclaiming.writes() {
            let mut disk = cut_at(&cut, &reclaiming.log, j);
            let room = disk.size;
            let mut v = Volume::open(&mut disk, 0, room).unwrap();
            assert_eq!(v.read_file("BIG.BIN", usize::MAX).unwrap(), big);
            assert_eq!(v.read_file("A.TXT", usize::MAX).unwrap(), pattern(3000, 1));
        }
        let mut done = reclaiming.disk;
        assert_eq!(
            check(&mut done),
            Ok(Usage {
                named: named(size),
                leaked: 0
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
        if freed > 0 {
            assert_eq!(done.u32_at(SECTOR as u64 + 488), CLUSTERS - named(size));
        }
    }
    assert!(leaks > 0, "no cut leaked anything, so nothing was tested");
}
