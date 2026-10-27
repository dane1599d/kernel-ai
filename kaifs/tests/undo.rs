//! Volumes, actions and undo, one case at a time (P2 task 6): the three volumes,
//! actions with their undo snapshots and UNDO records, abort, `restore(txn)`'s
//! rules, `changes` reading only what changed, and `history`. Task 6's exit:
//! a restore never overwrites a later change; `changes` reads only changed
//! subtrees; one commit spans volumes.

use kaifs::check::{Cache, check};
use kaifs::crash::{Options, Oracle, Recording, Rng, enumerate};
use kaifs::format;
use kaifs::fs::{HOME, MODELS, SYSTEM};
use kaifs::ns_check::check_namespace;
use kaifs::undo::Outcome;
use kaifs::vol::{SnapKind, UNDO_LIFE};
use kaifs::{Error, FileType, Fs, Image, MemDisk, ROOT, RecordingDisk};
use std::collections::BTreeMap;

const BLOCKS: u64 = 8192;

fn fresh() -> Fs<MemDisk> {
    Fs::mkfs(MemDisk::new(BLOCKS), [6; 16], "undo").unwrap()
}

fn remount(fs: Fs<MemDisk>) -> Fs<MemDisk> {
    Fs::mount(fs.into_dev()).unwrap()
}

fn checked<D: kaifs::BlockDev>(fs: &mut Fs<D>) {
    let mut cache = Cache::default();
    check(fs, &mut cache).unwrap();
    check_namespace(fs, &mut cache).unwrap();
}

fn split(path: &str) -> (&str, &str) {
    let at = path.rfind('/').unwrap();
    (if at == 0 { "/" } else { &path[..at] }, &path[at + 1..])
}

/// Writes `data` at `path`, making the file if needed; returns its inode.
fn write<D: kaifs::BlockDev>(fs: &mut Fs<D>, path: &str, data: &[u8]) -> u64 {
    let (dir, name) = split(path);
    let dir = fs.resolve(dir).unwrap();
    let ino = match fs.lookup(dir, name).unwrap() {
        Some(e) => e.ino,
        None => fs.create(dir, name).unwrap(),
    };
    fs.write_file(ino, data).unwrap();
    ino
}

fn read(fs: &mut Fs<MemDisk>, path: &str) -> Option<Vec<u8>> {
    let ino = fs.resolve(path).ok()?;
    fs.read_file(ino).ok()
}

fn mkdir(fs: &mut Fs<MemDisk>, path: &str) -> u64 {
    let (dir, name) = split(path);
    let dir = fs.resolve(dir).unwrap();
    fs.mkdir(dir, name).unwrap()
}

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
    let mut fs = fs.abort().unwrap();
    assert_eq!(fs.action(), None);
    assert_eq!(read(&mut fs, "/kept").unwrap(), b"committed");
    assert_eq!(fs.resolve("/new"), Err(Error::NotFound));
    fs.begin(2).unwrap();
    write(&mut fs, "/after", b"fine");
    fs.commit().unwrap();
    checked(&mut fs);
}

/// One action: `act` on volume `vol`, committed. Returns its generation.
fn action(fs: &mut Fs<MemDisk>, id: u64, act: impl FnOnce(&mut Fs<MemDisk>)) -> u64 {
    fs.begin(id).unwrap();
    act(fs);
    fs.commit().unwrap()
}

/// Restores `txn` as an action of its own; returns the outcomes and its commit.
fn undo(fs: &mut Fs<MemDisk>, txn: u64) -> (Vec<(u64, Outcome)>, u64) {
    fs.begin(9_000 + txn).unwrap();
    let out = fs.restore(txn).unwrap();
    (out, fs.commit().unwrap())
}

#[test]
fn restore_undoes_a_write_a_create_a_remove_and_a_rename() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let docs = mkdir(&mut fs, "/docs");
    let f = write(&mut fs, "/docs/report", b"draft 1");
    fs.set_xattr(f, "user.tag", b"v1").unwrap();
    fs.commit().unwrap();
    // A write.
    let t1 = action(&mut fs, 1, |fs| {
        fs.write_file(f, b"draft 2, much longer than the first")
            .unwrap();
        fs.set_xattr(f, "user.tag", b"v2").unwrap();
    });
    let (out, _) = undo(&mut fs, t1);
    assert!(out.contains(&(f, Outcome::Back)), "{out:?}");
    assert_eq!(read(&mut fs, "/docs/report").unwrap(), b"draft 1");
    assert_eq!(fs.get_xattr(f, "user.tag").unwrap().unwrap(), b"v1");
    // A create.
    let mut made = 0;
    let t2 = action(&mut fs, 2, |fs| made = write(fs, "/docs/new", b"fresh"));
    let (out, _) = undo(&mut fs, t2);
    assert!(out.contains(&(made, Outcome::Removed)), "{out:?}");
    assert_eq!(fs.resolve("/docs/new"), Err(Error::NotFound));
    // A remove: the same inode comes back.
    let t3 = action(&mut fs, 3, |fs| fs.remove(docs, "report").unwrap());
    let (out, _) = undo(&mut fs, t3);
    assert!(out.contains(&(f, Outcome::Back)), "{out:?}");
    assert_eq!(fs.resolve("/docs/report").unwrap(), f);
    assert_eq!(read(&mut fs, "/docs/report").unwrap(), b"draft 1");
    // A rename across folders, of a folder.
    let t4 = action(&mut fs, 4, |fs| {
        let archive = fs.mkdir(ROOT, "archive").unwrap();
        fs.rename(ROOT, "docs", archive, "old-docs").unwrap();
    });
    let (out, _) = undo(&mut fs, t4);
    assert!(out.contains(&(docs, Outcome::Back)), "{out:?}");
    assert_eq!(fs.resolve("/docs/report").unwrap(), f);
    assert_eq!(fs.resolve("/archive"), Err(Error::NotFound));
    fs.commit().unwrap();
    checked(&mut fs);
}

#[test]
fn restore_never_overwrites_a_later_change() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let a = write(&mut fs, "/a.txt", b"a, first");
    let b = write(&mut fs, "/b.txt", b"b, first");
    fs.commit().unwrap();
    let t1 = action(&mut fs, 1, |fs| {
        fs.write_file(a, b"a, by the AI").unwrap();
        fs.write_file(b, b"b, by the AI").unwrap();
    });
    // The user edits a afterwards, and renames b.
    action(&mut fs, 2, |fs| {
        fs.write_file(a, b"a, by the user").unwrap();
        fs.rename(ROOT, "b.txt", ROOT, "b-renamed.txt").unwrap();
    });
    let (out, _) = undo(&mut fs, t1);
    // Both were changed later: neither is touched, and each old version lands
    // beside it.
    assert_eq!(read(&mut fs, "/a.txt").unwrap(), b"a, by the user");
    assert_eq!(read(&mut fs, "/b-renamed.txt").unwrap(), b"b, by the AI");
    assert_eq!(read(&mut fs, "/a.txt (restored)").unwrap(), b"a, first");
    assert_eq!(read(&mut fs, "/b.txt (restored)").unwrap(), b"b, first");
    assert!(
        out.iter()
            .all(|(_, o)| matches!(o, Outcome::Beside { .. } | Outcome::Left)),
        "{out:?}"
    );
    fs.commit().unwrap();
    checked(&mut fs);
}

#[test]
fn a_restore_onto_a_taken_name_lands_beside_and_can_itself_be_undone() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let f = write(&mut fs, "/plan", b"the plan");
    fs.commit().unwrap();
    let t1 = action(&mut fs, 1, |fs| fs.remove(ROOT, "plan").unwrap());
    // Someone makes a new /plan after the removal.
    action(&mut fs, 2, |fs| {
        write(fs, "/plan", b"another plan");
    });
    let (out, t3) = undo(&mut fs, t1);
    let moved = Outcome::Moved {
        name: "plan (restored)".into(),
    };
    assert!(out.contains(&(f, moved)), "{out:?}");
    assert_eq!(read(&mut fs, "/plan").unwrap(), b"another plan");
    assert_eq!(read(&mut fs, "/plan (restored)").unwrap(), b"the plan");
    // The restore was an action too: undoing it removes what it put back.
    let (out, _) = undo(&mut fs, t3);
    assert!(out.contains(&(f, Outcome::Removed)), "{out:?}");
    assert_eq!(fs.resolve("/plan (restored)"), Err(Error::NotFound));
    assert_eq!(read(&mut fs, "/plan").unwrap(), b"another plan");
    fs.commit().unwrap();
    checked(&mut fs);
}

#[test]
fn a_removed_folder_comes_back_with_its_files() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    mkdir(&mut fs, "/photos");
    mkdir(&mut fs, "/photos/2026");
    write(&mut fs, "/photos/2026/one.jpg", &[1; 5000]);
    write(&mut fs, "/photos/two.jpg", &[2; 300]);
    fs.commit().unwrap();
    let t1 = action(&mut fs, 1, |fs| {
        let photos = fs.resolve("/photos").unwrap();
        let year = fs.resolve("/photos/2026").unwrap();
        fs.remove(year, "one.jpg").unwrap();
        fs.remove(photos, "2026").unwrap();
        fs.remove(photos, "two.jpg").unwrap();
        fs.remove(ROOT, "photos").unwrap();
    });
    assert_eq!(fs.resolve("/photos"), Err(Error::NotFound));
    undo(&mut fs, t1);
    assert_eq!(read(&mut fs, "/photos/2026/one.jpg").unwrap(), [1; 5000]);
    assert_eq!(read(&mut fs, "/photos/two.jpg").unwrap(), [2; 300]);
    checked(&mut fs);
    let mut fs = remount(fs);
    fs.set_volume(HOME).unwrap();
    assert_eq!(read(&mut fs, "/photos/two.jpg").unwrap(), [2; 300]);
}

/// What a reader sees of every live object of the active volume, by inode: the
/// folder and name it has, its data, and its attributes.
type Seen = BTreeMap<u64, ((u64, String), Option<Vec<u8>>, Vec<(String, Vec<u8>)>)>;

fn view(fs: &mut Fs<MemDisk>) -> Seen {
    let mut out = BTreeMap::new();
    out.insert(
        ROOT,
        ((ROOT, String::new()), None, fs.xattrs(ROOT).unwrap()),
    );
    let mut stack = vec![ROOT];
    while let Some(dir) = stack.pop() {
        for l in fs.list(dir, None, usize::MAX).unwrap() {
            let ino = l.entry.ino;
            let data = match l.entry.kind {
                FileType::Dir => {
                    stack.push(ino);
                    None
                }
                FileType::File => Some(fs.read_file(ino).unwrap()),
            };
            out.insert(ino, ((dir, l.entry.name), data, fs.xattrs(ino).unwrap()));
        }
    }
    out
}

/// Random actions, then restores of random earlier ones: whatever changed after
/// the restored action reads the same before and after the restore, and every
/// commit checks.
#[test]
fn random_restores_never_touch_a_later_change() {
    let names = ["a", "b", "c", "d", "e"];
    let mut rng = Rng(77);
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    mkdir(&mut fs, "/d1");
    mkdir(&mut fs, "/d2");
    fs.commit().unwrap();
    let mut actions: Vec<u64> = Vec::new();
    let rounds = if cfg!(debug_assertions) { 120 } else { 600 };
    let mut restored = 0;
    for round in 0..rounds {
        if !actions.is_empty() && rng.below(4) == 0 {
            let txn = actions[rng.below(actions.len() as u64) as usize];
            if fs.undo_record(txn).unwrap().is_none() {
                // The action changed nothing, so it left no record to restore.
                assert_eq!(fs.restore(txn), Err(Error::NotFound));
                continue;
            }
            let before = view(&mut fs);
            let later: Vec<u64> = before
                .keys()
                .copied()
                .filter(|&ino| fs.stat(ino).unwrap().changed > txn)
                .collect();
            fs.begin(10_000 + round).unwrap();
            fs.restore(txn).unwrap();
            let after = view(&mut fs);
            for ino in later {
                let (b, a) = (&before[&ino], after.get(&ino));
                // A folder changed later keeps its place and attributes; its
                // names may change as other objects come and go.
                assert_eq!(Some(b), a, "object {ino}, changed after {txn}, was touched");
            }
            actions.push(fs.commit().unwrap());
            restored += 1;
        } else {
            fs.begin(round).unwrap();
            for _ in 0..1 + rng.below(3) {
                let dir = ["/", "/d1", "/d2"][rng.below(3) as usize];
                let name = names[rng.below(5) as usize];
                let path = match dir {
                    "/" => format!("/{name}"),
                    _ => format!("{dir}/{name}"),
                };
                let d = fs.resolve(dir).unwrap();
                match rng.below(4) {
                    0 | 1 => {
                        let len = rng.below(3000) as usize;
                        let data: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
                        write(&mut fs, &path, &data);
                    }
                    2 => {
                        let _ = fs.remove(d, name);
                    }
                    _ => {
                        let to = fs
                            .resolve(["/", "/d1", "/d2"][rng.below(3) as usize])
                            .unwrap();
                        let _ = fs.rename(d, name, to, names[rng.below(5) as usize]);
                    }
                }
            }
            actions.push(fs.commit().unwrap());
        }
        if round % 20 == 0 {
            checked(&mut fs);
        }
    }
    checked(&mut fs);
    assert!(restored > rounds / 8, "only {restored} restores");
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
