//! The trash, retention and the GC, one case at a time (P2 task 6, part 2): a
//! trash is a move that its action's undo moves back; emptying it removes for
//! good; retention drops undo snapshots past 30 days, or the oldest under 10 %
//! free; the GC in slices frees exactly what nothing reaches.

use kaifs::check::{Cache, check};
use kaifs::fs::{Faults, HOME, SYSTEM};
use kaifs::gc::Why;
use kaifs::ns_check::check_namespace;
use kaifs::trash::{INFO, TRASH, TrashInfo, decode_info, encode_info};
use kaifs::vol::{SnapKind, UNDO_LIFE};
use kaifs::{Error, Fs, MemDisk, ROOT};

fn fresh(blocks: u64) -> Fs<MemDisk> {
    let mut fs = Fs::mkfs(MemDisk::new(blocks), [8; 16], "gc").unwrap();
    fs.set_volume(HOME).unwrap();
    fs
}

fn checked(fs: &mut Fs<MemDisk>) -> kaifs::check::Report {
    let mut cache = Cache::default();
    let report = check(fs, &mut cache).unwrap();
    check_namespace(fs, &mut cache).unwrap();
    report
}

fn write(fs: &mut Fs<MemDisk>, dir: u64, name: &str, data: &[u8]) -> u64 {
    let ino = match fs.lookup(dir, name).unwrap() {
        Some(e) => e.ino,
        None => fs.create(dir, name).unwrap(),
    };
    fs.write_file(ino, data).unwrap();
    ino
}

#[test]
fn a_trash_is_a_move_beside_info_cbor_and_its_undo_moves_it_back() {
    let mut fs = fresh(4096);
    let docs = fs.mkdir(ROOT, "docs").unwrap();
    let f = write(&mut fs, docs, "letter.txt", b"dear founder");
    fs.commit().unwrap();
    fs.clock = 5_000;
    fs.begin(31).unwrap();
    let trashed = fs.trash(docs, "letter.txt").unwrap();
    let txn = fs.commit().unwrap();
    assert_eq!(trashed.ino, f);
    assert_eq!(
        trashed.info,
        TrashInfo {
            path: "/docs/letter.txt".into(),
            deleted: 5_000,
            action: 31
        }
    );
    assert_eq!(fs.resolve("/docs/letter.txt"), Err(Error::NotFound));
    let inside = format!("/{TRASH}/{txn}/1/letter.txt");
    assert_eq!(fs.resolve(&inside).unwrap(), f);
    let info = fs.resolve(&format!("/{TRASH}/{txn}/1/{INFO}")).unwrap();
    assert_eq!(
        decode_info(&fs.read_file(info).unwrap()).unwrap(),
        trashed.info
    );
    assert_eq!(fs.trash_list().unwrap(), std::slice::from_ref(&trashed));
    assert!(matches!(fs.trash(ROOT, TRASH), Err(Error::Invalid(_))));
    // Undoing the action moves it back and takes the trash's folders away.
    fs.begin(32).unwrap();
    fs.restore(txn).unwrap();
    fs.commit().unwrap();
    assert_eq!(fs.resolve("/docs/letter.txt").unwrap(), f);
    assert_eq!(fs.read_file(f).unwrap(), b"dear founder");
    assert_eq!(fs.resolve(&format!("/{TRASH}/{txn}")), Err(Error::NotFound));
    assert!(fs.trash_list().unwrap().is_empty());
    checked(&mut fs);
}

#[test]
fn emptying_the_trash_removes_for_good_and_says_what_went() {
    let mut fs = fresh(4096);
    let d = fs.mkdir(ROOT, "d").unwrap();
    let inner = fs.mkdir(d, "inner").unwrap();
    write(&mut fs, inner, "deep", &[3; 7000]);
    write(&mut fs, ROOT, "a", b"a");
    write(&mut fs, ROOT, "b", b"b");
    fs.commit().unwrap();
    fs.begin(1).unwrap();
    fs.trash(ROOT, "d").unwrap();
    fs.trash(ROOT, "a").unwrap();
    fs.commit().unwrap();
    fs.begin(2).unwrap();
    fs.trash(ROOT, "b").unwrap();
    fs.commit().unwrap();
    let listed: Vec<String> = fs
        .trash_list()
        .unwrap()
        .into_iter()
        .map(|t| t.info.path)
        .collect();
    assert_eq!(listed, ["/d", "/a", "/b"]);
    let gone = fs.empty_trash().unwrap();
    assert_eq!(gone.len(), 3);
    fs.commit().unwrap();
    assert_eq!(fs.resolve(&format!("/{TRASH}")), Err(Error::NotFound));
    assert!(fs.trash_list().unwrap().is_empty());
    checked(&mut fs);
}

#[test]
fn info_cbor_skips_keys_it_does_not_know_and_refuses_what_is_not_it() {
    let info = TrashInfo {
        path: "/x".into(),
        deleted: u64::MAX,
        action: 0,
    };
    let bytes = encode_info(&info);
    assert_eq!(decode_info(&bytes).unwrap(), info);
    // A later writer's extra key, as a map of four.
    let mut longer = bytes.clone();
    longer[2] = 0xA4;
    longer.extend_from_slice(&[0x64, b'm', b'o', b'r', b'e', 0x82, 0x01, 0x62, b'h', b'i']);
    assert_eq!(decode_info(&longer).unwrap(), info);
    for bad in [&[][..], &[0x82, 0x02, 0xA0], &[0x81, 0x01], &[0xFF]] {
        assert!(decode_info(bad).is_err(), "{bad:?}");
    }
}

/// Actions that rewrite files leave every old block in an undo snapshot; once
/// the snapshots expire, a GC pass frees all of them, and nothing else.
#[test]
fn after_retention_a_gc_pass_frees_exactly_what_nothing_reaches() {
    let mut fs = fresh(8192);
    let files: Vec<u64> = (0..20)
        .map(|i| write(&mut fs, ROOT, &format!("f{i}"), &[i as u8; 20_000]))
        .collect();
    fs.commit().unwrap();
    fs.clock = 1_000;
    for round in 0..10u64 {
        fs.begin(round).unwrap();
        for f in &files {
            fs.write_file(*f, &[round as u8; 20_000]).unwrap();
        }
        fs.commit().unwrap();
    }
    let before = checked(&mut fs);
    assert_eq!(
        before.leaked_blocks, 0,
        "every old block is in an undo snapshot"
    );
    // Retention: nothing is 30 days old yet; then everything is.
    assert!(fs.expire(1_000 + UNDO_LIFE - 1).unwrap().is_empty());
    let dropped = fs.expire(1_000 + UNDO_LIFE).unwrap();
    assert_eq!(dropped.len(), 10);
    assert!(
        dropped
            .iter()
            .all(|d| d.why == Why::Expired && d.volume == HOME)
    );
    fs.commit().unwrap();
    let leaked = checked(&mut fs).leaked_blocks;
    assert!(leaked > 1_000, "{leaked} blocks are held by nothing");
    let freed = fs.gc(16).unwrap();
    assert_eq!(freed, leaked);
    fs.commit().unwrap();
    fs.commit().unwrap();
    let after = checked(&mut fs);
    assert_eq!(after.leaked_blocks, 0);
    assert!(after.free_blocks > before.free_blocks + 1_000);
    for f in &files {
        assert_eq!(fs.read_file(*f).unwrap(), [9; 20_000]);
    }
}

/// A pass in slices of two nodes, with actions that change things in between:
/// what was reachable when it began stays, the volume table's old nodes
/// included, and the next pass frees the rest.
#[test]
fn a_pass_in_slices_survives_commits_between_them() {
    let mut fs = fresh(8192);
    let files: Vec<u64> = (0..200)
        .map(|i| write(&mut fs, ROOT, &format!("f{i}"), &[1; 3000]))
        .collect();
    fs.commit().unwrap();
    // Enough actions that the volume table spans several nodes.
    for i in 0..60u64 {
        fs.begin(i).unwrap();
        fs.write_file(files[i as usize], &[3; 3000]).unwrap();
        fs.commit().unwrap();
    }
    assert!(
        fs.volumes_root().unwrap().level >= 1,
        "a volume table of one node"
    );
    fs.snapshot().unwrap();
    for f in &files {
        fs.write_file(*f, &[2; 3000]).unwrap();
    }
    fs.commit().unwrap();
    let snap = *fs.snapshots().keys().next().unwrap();
    fs.drop_snapshot(snap).unwrap();
    fs.commit().unwrap();
    fs.gc_start().unwrap();
    assert!(matches!(fs.gc_start(), Err(Error::Invalid(_))));
    let mut steps = 0;
    loop {
        let progress = fs.gc_step(2).unwrap();
        steps += 1;
        if progress.done {
            break;
        }
        // Between slices: the oldest undo snapshot goes, which rewrites the
        // volume table's oldest leaves, the ones the pass reads last; then an
        // action allocates, two commits past the wait for freed blocks.
        let oldest = fs.snapshots().keys().next().copied();
        if let Some(id) = oldest {
            fs.drop_snapshot(id).unwrap();
        }
        fs.commit().unwrap();
        let f = files[steps % files.len()];
        fs.begin(1_000 + steps as u64).unwrap();
        fs.write_file(f, &[steps as u8; 3000]).unwrap();
        fs.commit().unwrap();
        fs.commit().unwrap();
        if steps % 50 == 0 {
            checked(&mut fs);
        }
    }
    assert!(steps > 5, "the pass took {steps} slices");
    fs.commit().unwrap();
    fs.commit().unwrap();
    checked(&mut fs);
    fs.gc(1_000).unwrap();
    fs.commit().unwrap();
    fs.commit().unwrap();
    assert_eq!(checked(&mut fs).leaked_blocks, 0);
}

#[test]
fn under_a_tenth_free_retention_drops_the_oldest_undo_snapshot() {
    let mut fs = fresh(1024);
    let f = write(&mut fs, ROOT, "big", &[0; 4096]);
    fs.commit().unwrap();
    let mut round = 0u8;
    while !fs.space_low() {
        round += 1;
        fs.begin(u64::from(round)).unwrap();
        fs.write_file(f, &vec![round; 100_000]).unwrap();
        fs.commit().unwrap();
    }
    let oldest = fs
        .snapshots()
        .iter()
        .find(|(_, s)| s.kind == SnapKind::Undo)
        .map(|(id, _)| *id)
        .unwrap();
    let dropped = fs.expire(0).unwrap();
    assert_eq!(dropped.len(), 1);
    assert_eq!((dropped[0].snapshot, dropped[0].why), (oldest, Why::Space));
    fs.commit().unwrap();
    assert!(fs.gc(64).unwrap() > 0);
    fs.commit().unwrap();
    checked(&mut fs);
}

#[test]
fn a_pass_that_skips_snapshots_is_caught() {
    let mut fs = Fs::mkfs_with(
        MemDisk::new(4096),
        [9; 16],
        "fault",
        Faults {
            gc_skips_snapshots: true,
            ..Faults::default()
        },
    )
    .unwrap();
    fs.set_volume(SYSTEM).unwrap();
    let f = write(&mut fs, ROOT, "kept in a snapshot", &[5; 30_000]);
    fs.commit().unwrap();
    fs.snapshot().unwrap();
    fs.commit().unwrap();
    fs.write_file(f, &[6; 30_000]).unwrap();
    fs.commit().unwrap();
    fs.gc(64).unwrap();
    fs.commit().unwrap();
    fs.commit().unwrap();
    let mut cache = Cache::default();
    let found = check(&mut fs, &mut cache);
    assert!(
        found.is_err(),
        "the snapshot's blocks were freed and nothing noticed"
    );
}
