//! The engine's behaviour, one case at a time: format and mount, inline values and
//! records, scans, snapshots, restore and drop, the reuse rule's regression case
//! from spike S04-1, and the mount rules.

use kaifs::check::{Cache, check};
use kaifs::format::{self, Superblock};
use kaifs::{BlockDev, DevInfo, Error, Fs, Key, MemDisk, Value};

const BLOCKS: u64 = 8192;

fn fresh() -> Fs<MemDisk> {
    Fs::format(MemDisk::new(BLOCKS), [7; 16], "test").unwrap()
}

fn remount(fs: Fs<MemDisk>) -> Fs<MemDisk> {
    Fs::mount(fs.into_dev()).unwrap()
}

fn key(n: u64) -> Key {
    Key::new(n, 1, 0)
}

#[test]
fn a_fresh_filesystem_mounts_empty_and_checks_clean() {
    let mut fs = remount(fresh());
    assert_eq!(fs.committed, 1);
    assert_eq!(fs.get(&key(1)).unwrap(), None);
    let report = check(&mut fs, &mut Cache::default()).unwrap();
    assert_eq!((report.live, report.leaked_blocks), ((0, 0), 0));
}

#[test]
fn values_and_records_survive_a_remount() {
    let mut fs = fresh();
    fs.put(key(1), b"one").unwrap();
    let record: Vec<u8> = (0..10_000u32).map(|i| i as u8).collect();
    fs.put_record(key(2), &record).unwrap();
    fs.put(key(3), b"three").unwrap();
    assert!(fs.delete(&key(3)).unwrap());
    assert!(!fs.delete(&key(3)).unwrap());
    fs.commit().unwrap();
    let mut fs = remount(fs);
    assert_eq!(
        fs.get(&key(1)).unwrap(),
        Some(Value::Inline(b"one".to_vec()))
    );
    assert_eq!(fs.get(&key(2)).unwrap(), Some(Value::Record(record)));
    assert_eq!(fs.get(&key(3)).unwrap(), None);
    check(&mut fs, &mut Cache::default()).unwrap();
}

#[test]
fn an_uncommitted_change_is_gone_after_a_remount() {
    let mut fs = fresh();
    fs.put(key(1), b"kept").unwrap();
    fs.commit().unwrap();
    fs.put(key(1), b"lost").unwrap();
    let mut fs = remount(fs);
    assert_eq!(
        fs.get(&key(1)).unwrap(),
        Some(Value::Inline(b"kept".to_vec()))
    );
}

#[test]
fn a_scan_reads_in_order_across_batches() {
    let mut fs = fresh();
    for n in 0..1_000 {
        fs.put(Key::new(n / 7, (n % 7) as u8, n), &n.to_le_bytes())
            .unwrap();
    }
    fs.commit().unwrap();
    let mut seen = Vec::new();
    fs.scan(&Key::new(100, 0, 0), &mut |k, _| {
        seen.push(*k);
        true
    })
    .unwrap();
    assert_eq!(seen.len(), 1_000 - 700);
    assert!(seen.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn a_snapshot_keeps_its_state_and_restore_brings_it_back() {
    let mut fs = fresh();
    fs.put(key(1), b"before").unwrap();
    let id = fs.snapshot().unwrap();
    fs.put(key(1), b"after").unwrap();
    fs.put_record(key(2), &[5; 9000]).unwrap();
    fs.commit().unwrap();
    let mut fs = remount(fs);
    assert_eq!(fs.snapshots().len(), 1);
    let report = check(&mut fs, &mut Cache::default()).unwrap();
    assert_eq!(report.live.1, 2);
    assert_eq!(report.snaps[&id].1, 1);
    assert!(fs.restore(id).unwrap());
    fs.commit().unwrap();
    let mut fs = remount(fs);
    assert_eq!(
        fs.get(&key(1)).unwrap(),
        Some(Value::Inline(b"before".to_vec()))
    );
    assert_eq!(fs.get(&key(2)).unwrap(), None);
    assert!(fs.drop_snapshot(id).unwrap());
    fs.commit().unwrap();
    check(&mut remount(fs), &mut Cache::default()).unwrap();
}

/// S04-1's bug: snapshot S, commit; change the live tree (its frees wait, since S
/// holds those blocks); restore S; drop S; commit. Judged at commit, the waiting
/// frees were released while S's tree was live. Judged when replaced, they wait
/// for the GC and the restored tree stays whole.
#[test]
fn restore_then_drop_keeps_the_restored_blocks() {
    let mut fs = fresh();
    for n in 0..500 {
        fs.put(key(n), &[n as u8; 60]).unwrap();
    }
    fs.commit().unwrap();
    let id = fs.snapshot().unwrap();
    fs.commit().unwrap();
    for n in 0..500 {
        fs.put(key(n), &[0xEE; 60]).unwrap();
    }
    fs.restore(id).unwrap();
    fs.drop_snapshot(id).unwrap();
    fs.commit().unwrap();
    // Writes that would land on any wrongly freed block.
    for n in 1000..1600 {
        fs.put(key(n), &[1; 60]).unwrap();
        if n % 100 == 0 {
            fs.commit().unwrap();
        }
    }
    fs.commit().unwrap();
    let mut fs = remount(fs);
    for n in 0..500 {
        assert_eq!(
            fs.get(&key(n)).unwrap(),
            Some(Value::Inline(vec![n as u8; 60]))
        );
    }
    // The held blocks are the restored tree's own, live again: nothing leaked.
    let report = check(&mut fs, &mut Cache::default()).unwrap();
    assert_eq!(report.live.1, 1100);
}

/// The same with the rule judged at commit: the restored tree breaks, while the
/// filesystem runs or when it is checked.
#[test]
fn judging_at_commit_breaks_the_restored_tree() {
    let faults = kaifs::fs::Faults {
        decide_at_commit: true,
        ..Default::default()
    };
    let broken = (|| -> Result<(), String> {
        let e = |e: Error| format!("{e:?}");
        let mut fs = Fs::format_with(MemDisk::new(BLOCKS), [7; 16], "test", faults).map_err(e)?;
        for n in 0..500 {
            fs.put(key(n), &[n as u8; 60]).map_err(e)?;
        }
        fs.commit().map_err(e)?;
        let id = fs.snapshot().map_err(e)?;
        fs.commit().map_err(e)?;
        for n in 0..500 {
            fs.put(key(n), &[0xEE; 60]).map_err(e)?;
        }
        fs.restore(id).map_err(e)?;
        fs.drop_snapshot(id).map_err(e)?;
        fs.commit().map_err(e)?;
        for n in 1000..1600 {
            fs.put(key(n), &[1; 60]).map_err(e)?;
            if n % 100 == 0 {
                fs.commit().map_err(e)?;
            }
        }
        fs.commit().map_err(e)?;
        let mut fs = Fs::mount(fs.into_dev()).map_err(e)?;
        check(&mut fs, &mut Cache::default())?;
        for n in 0..500 {
            if fs.get(&key(n)).map_err(e)? != Some(Value::Inline(vec![n as u8; 60])) {
                return Err("a restored value changed".into());
            }
        }
        Ok(())
    })();
    assert!(broken.is_err(), "the faulty rule went unnoticed");
}

/// A device that says it caches writes and cannot flush.
struct NoFlush(MemDisk);

impl BlockDev for NoFlush {
    fn info(&self) -> DevInfo {
        DevInfo {
            volatile_cache: true,
            flush: false,
            ..self.0.info()
        }
    }
    fn read(&mut self, addr: u64, buf: &mut kaifs::Block) -> Result<(), kaifs::IoError> {
        self.0.read(addr, buf)
    }
    fn write(&mut self, addr: u64, data: &kaifs::Block) -> Result<(), kaifs::IoError> {
        self.0.write(addr, data)
    }
    fn flush(&mut self) -> Result<(), kaifs::IoError> {
        Ok(())
    }
}

#[test]
fn a_cache_that_cannot_flush_is_refused_and_mounts_read_only() {
    assert!(matches!(
        Fs::format(NoFlush(MemDisk::new(BLOCKS)), [1; 16], "x"),
        Err(Error::Unsupported(_))
    ));
    let disk = fresh().into_dev();
    let mut fs = Fs::mount(NoFlush(disk)).unwrap();
    assert!(!fs.writable());
    assert_eq!(fs.commit(), Err(Error::ReadOnly));
}

#[test]
fn an_unknown_ro_compat_feature_mounts_read_only() {
    let mut disk = fresh().into_dev();
    let found = format::read_rings(&mut disk).unwrap();
    let mut sb: Superblock = found[0];
    sb.ro_compat = 1 << 5;
    sb.generation += 1;
    format::write_rings(&mut disk, &sb).unwrap();
    let mut fs = Fs::mount(disk).unwrap();
    assert!(!fs.writable());
    assert_eq!(fs.commit(), Err(Error::ReadOnly));
}
