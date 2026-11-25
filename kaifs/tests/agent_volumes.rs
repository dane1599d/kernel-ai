//! ADR 0013's `memory` and `index` volumes (agents-os.md M1, S1): added to a
//! disk that lacks them and kept across a remount; `memory`'s undo snapshots
//! live an hour and a purge drops every snapshot holding a record; `index`
//! takes no snapshot at all.

use kaifs::check::{Cache, check};
use kaifs::fs::{HOME, VOLUMES};
use kaifs::ns_check::check_namespace;
use kaifs::vol::{AGENT_VOLUMES, INDEX, MEMORY, SnapKind, UNDO_LIFE};
use kaifs::{Fs, MemDisk, ROOT};

const HOUR: u64 = 3600 * 1_000_000_000;

fn fresh() -> Fs<MemDisk> {
    let mut fs = Fs::mkfs(MemDisk::new(4096), [3; 16], "agents").unwrap();
    assert_eq!(fs.add_agent_volumes().unwrap(), 2);
    fs.commit().unwrap();
    fs
}

fn checked(fs: &mut Fs<MemDisk>) {
    check(fs, &mut Cache::default()).unwrap();
    check_namespace(fs, &mut Cache::default()).unwrap();
}

#[test]
fn mkfs_makes_neither_and_a_disk_gains_both_once() {
    let mut fs = Fs::mkfs(MemDisk::new(4096), [3; 16], "agents").unwrap();
    let names = |fs: &Fs<MemDisk>| fs.volumes().into_iter().map(|(_, n)| n).collect::<Vec<_>>();
    assert_eq!(names(&fs), VOLUMES.map(|(_, n)| n.to_string()));
    assert_eq!(fs.add_agent_volumes().unwrap(), 2);
    fs.commit().unwrap();
    let mut fs = Fs::mount(fs.into_dev()).unwrap();
    for (id, name) in AGENT_VOLUMES {
        assert!(fs.volumes().contains(&(id, name.to_string())), "{name}");
        fs.set_volume(id).unwrap();
        assert!(fs.list(ROOT, None, usize::MAX).unwrap().is_empty());
    }
    assert_eq!(fs.add_agent_volumes().unwrap(), 0);
    checked(&mut fs);
}

#[test]
fn memory_keeps_undo_snapshots_an_hour_and_home_its_thirty_days() {
    let mut fs = fresh();
    fs.clock = 1_000;
    fs.begin(1).unwrap();
    for volume in [MEMORY, HOME] {
        fs.set_volume(volume).unwrap();
        let f = fs.create(ROOT, "note").unwrap();
        fs.write_file(f, b"likes tea").unwrap();
    }
    fs.commit().unwrap();
    let expiry = |fs: &mut Fs<MemDisk>, volume| {
        fs.set_volume(volume).unwrap();
        let undo: Vec<u64> = fs
            .snapshots()
            .values()
            .filter(|s| s.kind == SnapKind::Undo)
            .map(|s| s.expiry)
            .collect();
        undo
    };
    assert_eq!(expiry(&mut fs, MEMORY), [1_000 + HOUR]);
    assert_eq!(expiry(&mut fs, HOME), [1_000 + UNDO_LIFE]);
    // An hour on, only memory's has expired, whatever the free space.
    assert!(!fs.expiry_due(1_000 + HOUR - 1));
    assert!(fs.expiry_due(1_000 + HOUR));
    let dropped = fs.expire(1_000 + HOUR).unwrap();
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].volume, MEMORY);
    fs.commit().unwrap();
    assert!(expiry(&mut fs, MEMORY).is_empty());
    assert_eq!(expiry(&mut fs, HOME).len(), 1);
    checked(&mut fs);
}

#[test]
fn index_is_never_backed_up() {
    let mut fs = fresh();
    fs.begin(1).unwrap();
    fs.set_volume(INDEX).unwrap();
    let f = fs.create(ROOT, "chunks").unwrap();
    fs.write_file(f, &[7; 5000]).unwrap();
    fs.set_volume(HOME).unwrap();
    fs.create(ROOT, "doc.txt").unwrap();
    let generation = fs.commit().unwrap();
    fs.set_volume(INDEX).unwrap();
    assert!(fs.snapshots().is_empty(), "an undo snapshot of index");
    // The action's undo record and snapshot are home's alone.
    assert!(fs.undo_record(generation).unwrap().is_none());
    fs.set_volume(HOME).unwrap();
    assert!(fs.undo_record(generation).unwrap().is_some());
    checked(&mut fs);
}

#[test]
fn a_purge_drops_every_memory_snapshot_holding_the_record_and_no_other() {
    let mut fs = fresh();
    fs.set_volume(MEMORY).unwrap();
    // Action 1 makes the record: its undo snapshot is from before it.
    fs.begin(1).unwrap();
    let record = fs.create(ROOT, "r1").unwrap();
    fs.write_file(record, b"my sister is Ana").unwrap();
    fs.commit().unwrap();
    // A named snapshot, and action 2 forgets it: both hold the record.
    let named = fs.snapshot().unwrap();
    fs.commit().unwrap();
    fs.begin(2).unwrap();
    fs.remove(ROOT, "r1").unwrap();
    fs.commit().unwrap();
    let holding = |fs: &mut Fs<MemDisk>| {
        let roots: Vec<_> = fs.snapshots().iter().map(|(id, s)| (*id, s.root)).collect();
        roots
            .into_iter()
            .filter(|(_, root)| {
                fs.get_raw_in(
                    root,
                    &kaifs::tree::Key::new(record, kaifs::items::K_INODE, 0),
                )
                .unwrap()
                .is_some()
            })
            .count()
    };
    assert_eq!(fs.snapshots().len(), 3);
    assert_eq!(holding(&mut fs), 2);
    assert!(fs.snapshots().contains_key(&named));
    assert_eq!(fs.purge(record).unwrap(), 2);
    fs.commit().unwrap();
    // After a reboot no snapshot holds it, and the one from before it stays.
    let mut fs = Fs::mount(fs.into_dev()).unwrap();
    fs.set_volume(MEMORY).unwrap();
    assert_eq!(holding(&mut fs), 0);
    assert_eq!(fs.snapshots().len(), 1);
    assert!(fs.lookup(ROOT, "r1").unwrap().is_none());
    checked(&mut fs);
}

#[test]
fn dropping_the_undo_snapshots_holding_a_file_keeps_every_other_kind() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    fs.begin(1).unwrap();
    let blob = fs.create(ROOT, "blob").unwrap();
    fs.write_file(blob, &[9; 9000]).unwrap();
    fs.commit().unwrap();
    // A named snapshot and action 2's undo snapshot both hold it.
    let named = fs.snapshot().unwrap();
    fs.commit().unwrap();
    fs.begin(2).unwrap();
    fs.create(ROOT, "later").unwrap();
    fs.commit().unwrap();
    assert_eq!(fs.snapshots().len(), 3);
    fs.remove(ROOT, "blob").unwrap();
    // Only the undo snapshot that holds it goes: not action 1's, from
    // before it, and not the named one.
    assert_eq!(fs.drop_undo_holding(HOME, blob).unwrap(), 1);
    fs.commit().unwrap();
    let mut fs = Fs::mount(fs.into_dev()).unwrap();
    fs.set_volume(HOME).unwrap();
    assert_eq!(fs.snapshots().len(), 2);
    assert!(fs.snapshots().contains_key(&named));
    checked(&mut fs);
}
