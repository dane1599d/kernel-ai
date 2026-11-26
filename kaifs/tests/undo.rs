//! Volumes, actions and undo, one case at a time (P2 task 6): the three volumes,
//! actions with their undo snapshots and UNDO records, abort, `changes` reading
//! only what changed, and `history`; `restore(txn)`'s rules are in
//! undo_restore.rs. Task 6's exit: a restore never overwrites a later change;
//! `changes` reads only changed subtrees; one commit spans volumes.

use kaifs::check::{Cache, check};
use kaifs::crash::{Options, Oracle, Recording, Rng, enumerate};
use kaifs::format;
use kaifs::fs::{HOME, MODELS, SYSTEM};
use kaifs::ns_check::check_namespace;
use kaifs::vol::{SnapKind, UNDO_LIFE};
use kaifs::{Error, FileType, Fs, Image, MemDisk, ROOT, RecordingDisk};

mod undo_kit;
use undo_kit::*;

#[test]
fn mkfs_makes_three_volumes_each_with_its_own_namespace() {
    let mut fs = fresh();
    let names: Vec<String> = fs.volumes().into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, ["system", "home", "models"]);
    assert_eq!(fs.volume(), SYSTEM);
    write(&mut fs, "/a", b"system's");
    fs.set_volume(HOME).unwrap();
    assert_eq!(fs.resolve("/a"), Err(Error::NotFound));
    write(&mut fs, "/a", b"home's");
    assert_eq!(fs.set_volume(9), Err(Error::NotFound));
    fs.commit().unwrap();
    let mut fs = remount(fs);
    assert_eq!(read(&mut fs, "/a").unwrap(), b"system's");
    fs.set_volume(HOME).unwrap();
    assert_eq!(read(&mut fs, "/a").unwrap(), b"home's");
    fs.set_volume(MODELS).unwrap();
    assert_eq!(fs.stat(ROOT).unwrap().kind, FileType::Dir);
    checked(&mut fs);
}

#[test]
fn an_action_writes_an_undo_snapshot_and_record_for_each_volume_it_changed() {
    let mut fs = fresh();
    fs.clock = 1_000;
    fs.set_volume(HOME).unwrap();
    let before = fs.committed;
    fs.begin(41).unwrap();
    assert!(matches!(fs.begin(42), Err(Error::Invalid(_))));
    let f = write(&mut fs, "/notes", b"one");
    fs.set_volume(SYSTEM).unwrap();
    write(&mut fs, "/log", b"wrote notes");
    let txn = fs.commit().unwrap();
    assert_eq!(fs.action(), None);
    for (vol, object) in [(HOME, f), (SYSTEM, fs.resolve("/log").unwrap())] {
        fs.set_volume(vol).unwrap();
        let undo = fs.undo_record(txn).unwrap().unwrap();
        assert_eq!((undo.before, undo.action, undo.time), (before, 41, 1_000));
        assert!(undo.objects.contains(&object) && undo.objects.contains(&ROOT));
        let snap = fs.snapshots()[&undo.snapshot];
        assert_eq!(
            (snap.kind, snap.generation, snap.pin),
            (SnapKind::Undo, before, 41)
        );
        assert_eq!(snap.expiry, 1_000 + UNDO_LIFE);
    }
    fs.set_volume(MODELS).unwrap();
    assert!(
        fs.snapshots().is_empty(),
        "an untouched volume gets no undo snapshot"
    );
    // Changes outside an action must be committed before one begins.
    fs.set_volume(HOME).unwrap();
    write(&mut fs, "/loose", b"x");
    assert!(matches!(fs.begin(43), Err(Error::Invalid(_))));
    fs.commit().unwrap();
    let mut fs = remount(fs);
    fs.set_volume(HOME).unwrap();
    assert_eq!(fs.undo_records().unwrap().len(), 1);
    checked(&mut fs);
}

#[test]
fn abort_drops_the_open_action_and_keeps_the_last_commit() {
    let mut fs = fresh();
    write(&mut fs, "/kept", b"committed");
    fs.commit().unwrap();
    fs.begin(1).unwrap();
    write(&mut fs, "/kept", b"changed");
    write(&mut fs, "/new", b"made");
    fs.set_volume(HOME).unwrap();
    fs.abort().unwrap();
    assert_eq!(fs.action(), None);
    assert_eq!(fs.volume(), HOME, "the volume calls act on is kept");
    fs.set_volume(SYSTEM).unwrap();
    assert_eq!(read(&mut fs, "/kept").unwrap(), b"committed");
    assert_eq!(fs.resolve("/new"), Err(Error::NotFound));
    fs.begin(2).unwrap();
    write(&mut fs, "/after", b"fine");
    fs.commit().unwrap();
    checked(&mut fs);
}

/// A disk whose writes fail once `fail_after` more have gone through.
struct Failing {
    disk: MemDisk,
    fail_after: u64,
}

impl kaifs::BlockDev for Failing {
    fn info(&self) -> kaifs::DevInfo {
        self.disk.info()
    }

    fn read(&mut self, addr: u64, buf: &mut kaifs::Block) -> Result<(), kaifs::IoError> {
        self.disk.read(addr, buf)
    }

    fn write(&mut self, addr: u64, data: &kaifs::Block) -> Result<(), kaifs::IoError> {
        if self.fail_after == 0 {
            return Err(kaifs::IoError);
        }
        self.fail_after -= 1;
        self.disk.write(addr, data)
    }

    fn flush(&mut self) -> Result<(), kaifs::IoError> {
        self.disk.flush()
    }
}

/// An I/O error at any write of an action's commit leaves the `Fs` in a state no
/// one should use; `abort` reads back the newest commit on the disk, in place:
/// the last one, or the interrupted one if it landed whole, and nothing between.
/// Work then goes on.
#[test]
fn abort_after_an_io_error_midway_reads_back_a_whole_commit() {
    let disk = Failing {
        disk: MemDisk::new(BLOCKS),
        fail_after: u64::MAX,
    };
    let mut fs = Fs::mkfs(disk, [4; 16], "failing").unwrap();
    fs.set_volume(HOME).unwrap();
    let f = write(&mut fs, "/kept", &[0; 20_000]);
    let mut kept = fs.commit().unwrap();
    let (mut rolled_back, mut landed) = (0, 0);
    for budget in 0u64.. {
        let round = budget as u8 + 1;
        fs.begin(budget + 1).unwrap();
        fs.write_file(f, &[round; 20_000]).unwrap();
        write(&mut fs, &format!("/half {budget}"), &[round; 9_000]);
        fs.io.dev.fail_after = budget;
        let committed = fs.commit();
        fs.io.dev.fail_after = u64::MAX;
        if committed.is_ok() {
            break;
        }
        assert_eq!(committed, Err(Error::Io));
        let read = fs.abort().unwrap();
        assert_eq!(read, fs.committed);
        let half = fs.resolve(&format!("/half {budget}"));
        match read == kept {
            true => {
                rolled_back += 1;
                assert_ne!(fs.read_file(f).unwrap(), [round; 20_000]);
                assert_eq!(half, Err(Error::NotFound));
            }
            false => {
                landed += 1;
                assert_eq!(read, kept + 1);
                assert_eq!(fs.read_file(f).unwrap(), [round; 20_000]);
                assert!(half.is_ok());
                kept = read;
            }
        }
        let mut cache = Cache::default();
        check(&mut fs, &mut cache).unwrap();
        check_namespace(&mut fs, &mut cache).unwrap();
    }
    assert!(rolled_back >= 3, "{rolled_back} commits rolled back");
    assert!(landed >= 1, "no error came after a whole superblock");
    fs.begin(99).unwrap();
    fs.write_file(f, b"after the errors").unwrap();
    fs.commit().unwrap();
    let mut fs = Fs::mount(fs.into_dev()).unwrap();
    fs.set_volume(HOME).unwrap();
    assert_eq!(read_on(&mut fs, "/kept").unwrap(), b"after the errors");
}

#[test]
fn changes_reads_only_the_subtrees_that_changed() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let mut files = Vec::new();
    for d in 0..20 {
        let dir = mkdir(&mut fs, &format!("/dir{d}"));
        for f in 0..100 {
            let ino = fs.create(dir, &format!("file{f}")).unwrap();
            fs.write_file(ino, &[d as u8; 200]).unwrap();
            files.push(ino);
        }
    }
    let since = fs.commit().unwrap();
    let target = files[1234];
    fs.write_file(target, b"changed").unwrap();
    fs.commit().unwrap();
    let mut fs = remount(fs);
    fs.set_volume(HOME).unwrap();
    // The whole tree, for scale.
    let reads = fs.io.node_reads;
    fs.scan_raw(&kaifs::Key::MIN, &mut |_, _| true).unwrap();
    let all = fs.io.node_reads - reads;
    let reads = fs.io.node_reads;
    let changed = fs.changes(since).unwrap();
    let walked = fs.io.node_reads - reads;
    assert_eq!(
        changed.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        [target]
    );
    let height = u64::from(fs.live_root().unwrap().level) + 1;
    assert!(
        walked <= height,
        "changes read {walked} nodes; the path is {height}"
    );
    assert!(all > 20 * walked, "the tree has {all} nodes");
    eprintln!("kaifs changes: 1 file of 2,000 changed: {walked} of {all} nodes read");
    // Nothing changed since the last commit: not even the root is read.
    let reads = fs.io.node_reads;
    assert!(fs.changes(fs.committed).unwrap().is_empty());
    assert_eq!(fs.io.node_reads, reads);
}

#[test]
fn history_lists_each_version_once_and_reads_it_back() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let f = write(&mut fs, "/essay", b"version 1");
    fs.commit().unwrap();
    for (i, text) in [&b"version 2"[..], b"version 3", b"version 3"]
        .iter()
        .enumerate()
    {
        action(&mut fs, i as u64 + 1, |fs| {
            if i < 2 {
                fs.write_file(f, text).unwrap();
            } else {
                write(fs, "/other", b"leaves the essay alone");
            }
        });
    }
    let versions = fs.history(f).unwrap();
    let texts: Vec<Vec<u8>> = versions
        .iter()
        .map(|v| fs.read_version(v.snapshot, f, 0, 100).unwrap())
        .collect();
    assert_eq!(texts, [b"version 2".to_vec(), b"version 1".to_vec()]);
    assert_eq!(
        fs.path(Some(versions[0].snapshot), f).unwrap().unwrap(),
        "/essay"
    );
}

#[test]
fn dropping_an_undo_snapshot_drops_its_record() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let txn = action(&mut fs, 1, |fs| {
        write(fs, "/x", b"x");
    });
    let undo = fs.undo_record(txn).unwrap().unwrap();
    assert!(fs.drop_snapshot(undo.snapshot).unwrap());
    assert_eq!(fs.undo_record(txn).unwrap(), None);
    assert_eq!(fs.restore(txn), Err(Error::NotFound));
    fs.commit().unwrap();
    checked(&mut fs);
}

/// Each crash state of an action that changes two volumes shows both changes or
/// neither.
struct BothOrNeither {
    states: u64,
}

impl Oracle for BothOrNeither {
    fn judge(&mut self, image: &mut Image<'_>, _: u64) -> Result<(), String> {
        self.states += 1;
        let mut fs = Fs::mount(&mut *image).map_err(|e| format!("mount: {e:?}"))?;
        let mut seen = Vec::new();
        for vol in [HOME, SYSTEM] {
            fs.set_volume(vol).unwrap();
            let found = fs.resolve(["/report", "/log"][usize::from(vol == SYSTEM)]);
            seen.push(found.is_ok());
            if let Ok(ino) = found {
                let data = fs.read_file(ino).map_err(|e| format!("{e:?}"))?;
                if data.len() != 3000 {
                    return Err("a half-written file".into());
                }
            }
        }
        let mut cache = Cache::default();
        check(&mut fs, &mut cache)?;
        check_namespace(&mut fs, &mut cache)?;
        match seen[0] == seen[1] {
            true => Ok(()),
            false => Err(format!("one volume's change without the other's: {seen:?}")),
        }
    }
}

#[test]
fn one_commit_spans_volumes() {
    let mut fs = Fs::mkfs(RecordingDisk::new(4096), [7; 16], "spans").unwrap();
    fs.io.dev.checkpoint();
    let base = fs.io.dev.disk.clone();
    fs.begin(5).unwrap();
    fs.set_volume(HOME).unwrap();
    write(&mut fs, "/report", &[7; 3000]);
    fs.set_volume(SYSTEM).unwrap();
    write(&mut fs, "/log", &[8; 3000]);
    let txn = fs.commit().unwrap();
    let slots: Vec<u64> = format::all_slots(fs.block_count()).collect();
    let tear = |addr: u64| slots.contains(&addr);
    let waited = [(0, txn - 1)];
    let recording = Recording {
        base: &base,
        log: &fs.io.dev.log,
        waited: &waited,
        tear: &tear,
    };
    let mut oracle = BothOrNeither { states: 0 };
    let tally = enumerate(&recording, &Options::default(), &mut oracle, &mut Rng(3));
    eprintln!(
        "kaifs one commit, two volumes: {} crash states, {} violations",
        tally.states, tally.violations
    );
    assert_eq!(tally.violations, 0, "{:#?}", tally.examples);
    assert!(tally.states > 50);
}
