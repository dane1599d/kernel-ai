//! The core on its own: format, write, commit, mount, check, snapshots, GC.

use spike_kaifs::check::{self, Cache};
use spike_kaifs::{CsumKind, Fs, MemDisk, Value};

fn fresh(blocks: u64) -> Fs<MemDisk> {
    Fs::format(MemDisk::new(blocks), CsumKind::Sha256, [7; 16]).unwrap()
}

fn remount(fs: Fs<MemDisk>) -> Fs<MemDisk> {
    Fs::mount(fs.into_dev()).unwrap()
}

#[test]
fn values_survive_a_remount() {
    let mut fs = fresh(4096);
    for key in 0..2000u64 {
        fs.put(key, &key.to_le_bytes().repeat(8)).unwrap();
    }
    fs.put_record(5000, &[9; 20_000]).unwrap();
    let generation = fs.commit().unwrap();
    let mut fs = remount(fs);
    assert_eq!(fs.committed, generation);
    assert_eq!(
        fs.get(17).unwrap(),
        Some(Value::Inline(17u64.to_le_bytes().repeat(8)))
    );
    assert_eq!(fs.get(5000).unwrap(), Some(Value::Record(vec![9; 20_000])));
    assert_eq!(fs.get(99_999).unwrap(), None);
    let report = check::check(&mut fs, &mut Cache::default()).unwrap();
    assert_eq!(report.live.1, 2001);
    assert_eq!(report.leaked_blocks, 0);
}

#[test]
fn deletes_empty_the_tree() {
    let mut fs = fresh(4096);
    for key in 0..1000u64 {
        fs.put(key, &[1; 40]).unwrap();
    }
    fs.commit().unwrap();
    for key in 0..1000u64 {
        assert!(fs.delete(key).unwrap());
    }
    assert!(!fs.delete(3).unwrap());
    fs.commit().unwrap();
    let mut fs = remount(fs);
    let report = check::check(&mut fs, &mut Cache::default()).unwrap();
    assert_eq!(report.live, (0, 0));
}

#[test]
fn snapshots_restore_and_gc() {
    let mut fs = fresh(8192);
    for key in 0..500u64 {
        fs.put(key, b"first").unwrap();
    }
    fs.commit().unwrap();
    let snap = fs.snapshot().unwrap();
    fs.commit().unwrap();
    for key in 0..500u64 {
        fs.put(key, b"second").unwrap();
    }
    fs.commit().unwrap();
    let before = check::check(&mut fs, &mut Cache::default()).unwrap();
    assert_ne!(before.live, before.snaps[&snap]);
    assert!(fs.restore(snap).unwrap());
    fs.commit().unwrap();
    assert_eq!(fs.get(3).unwrap(), Some(Value::Inline(b"first".to_vec())));
    assert!(fs.drop_snapshot(snap).unwrap());
    fs.commit().unwrap();
    let report = check::check(&mut fs, &mut Cache::default()).unwrap();
    assert!(
        report.leaked_blocks > 0,
        "the second tree's blocks wait for the GC"
    );
    let reclaimed = check::gc(&mut fs).unwrap();
    assert_eq!(reclaimed, report.leaked_blocks);
    fs.commit().unwrap();
    let mut fs = remount(fs);
    let after = check::check(&mut fs, &mut Cache::default()).unwrap();
    assert_eq!(after.leaked_blocks, 0);
    assert_eq!(after.live, before.snaps[&snap]);
}
