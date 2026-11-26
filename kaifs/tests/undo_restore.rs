//! `restore(txn)`'s rules, one case at a time (P2 task 6; ADR 0004 §4): a
//! restore never overwrites a later change, puts back what the action changed
//! or removed, and removes what it made.

mod undo_kit;

use kaifs::crash::Rng;
use kaifs::fs::HOME;
use kaifs::undo::Outcome;
use kaifs::{Error, FileType, Fs, MemDisk, ROOT};
use std::collections::BTreeMap;
use undo_kit::*;

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
            .all(|(_, o)| matches!(o, Outcome::Beside { .. } | Outcome::Left { .. })),
        "{out:?}"
    );
    fs.commit().unwrap();
    checked(&mut fs);
}

/// bug-hunt-6 H6-23: one entry's changes in two actions, undone newest first.
/// The restore of the second puts the file back as the first left it, so the
/// first's restore finds no version it would lose and puts the file back too,
/// with nothing written beside it. Before, the second's restore stamped the
/// file later than the first, which then left it half undone.
#[test]
fn restoring_two_actions_of_one_change_newest_first_puts_the_file_back() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let todo = write(&mut fs, "/todo.txt", b"orig");
    fs.commit().unwrap();
    let t1 = action(&mut fs, 1, |fs| fs.write_file(todo, b"a").unwrap());
    let t2 = action(&mut fs, 2, |fs| fs.write_file(todo, b"b").unwrap());
    let (out, _) = undo(&mut fs, t2);
    assert_eq!(out, [(todo, Outcome::Back)]);
    assert_eq!(read(&mut fs, "/todo.txt").unwrap(), b"a");
    let (out, _) = undo(&mut fs, t1);
    assert!(out.contains(&(todo, Outcome::Back)), "{out:?}");
    assert_eq!(read(&mut fs, "/todo.txt").unwrap(), b"orig");
    assert_eq!(read(&mut fs, "/todo.txt (restored)"), None);
    checked(&mut fs);
}

/// The same when the first action made the file: undone, it is gone again.
#[test]
fn a_file_one_action_made_and_the_next_wrote_is_removed_by_undoing_both() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let t1 = action(&mut fs, 1, |fs| {
        write(fs, "/log.txt", b"one");
    });
    let log = fs.resolve("/log.txt").unwrap();
    let t2 = action(&mut fs, 2, |fs| fs.write_file(log, b"two").unwrap());
    undo(&mut fs, t2);
    let (out, _) = undo(&mut fs, t1);
    assert!(out.contains(&(log, Outcome::Removed)), "{out:?}");
    assert_eq!(read(&mut fs, "/log.txt"), None);
    checked(&mut fs);
}

/// A later change that only added a name to the folder an action made a file
/// in leaves that folder, and says it is a folder: the file is gone again and
/// every file is back, so a caller counting what stays changed counts none
/// (#1442's review).
#[test]
fn a_folder_left_for_a_later_name_is_told_apart_from_a_file() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let docs = mkdir(&mut fs, "/docs");
    fs.commit().unwrap();
    let t1 = action(&mut fs, 1, |fs| {
        write(fs, "/docs/a.txt", b"one");
    });
    let a = fs.resolve("/docs/a.txt").unwrap();
    action(&mut fs, 2, |fs| {
        write(fs, "/docs/b.txt", b"two");
    });
    let (out, _) = undo(&mut fs, t1);
    assert!(out.contains(&(a, Outcome::Removed)), "{out:?}");
    assert!(
        out.contains(&(docs, Outcome::Left { folder: true })),
        "{out:?}"
    );
    assert!(
        !out.contains(&(docs, Outcome::Left { folder: false })),
        "{out:?}"
    );
    assert_eq!(read(&mut fs, "/docs/a.txt"), None);
    assert_eq!(read(&mut fs, "/docs/b.txt").unwrap(), b"two");
    checked(&mut fs);
}

/// A file whose data items are raw-alike is put back with no item decoded;
/// one whose data differs only past its first item is still told changed.
#[test]
fn a_later_write_past_the_first_item_still_counts_as_a_change() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let f = write(&mut fs, "/big.bin", &big);
    fs.commit().unwrap();
    let mut mine = big.clone();
    mine[0] = 1;
    let t1 = action(&mut fs, 1, |fs| fs.write_file(f, &mine).unwrap());
    let mut later = mine.clone();
    *later.last_mut().unwrap() ^= 0xFF;
    let t2 = action(&mut fs, 2, |fs| fs.write_file(f, &later).unwrap());
    // The second's restore puts back what the first left: alike, so the
    // first's restore puts the file back whole.
    undo(&mut fs, t2);
    let (out, _) = undo(&mut fs, t1);
    assert!(out.contains(&(f, Outcome::Back)), "{out:?}");
    assert_eq!(read(&mut fs, "/big.bin").unwrap(), big);
    // A change past the first item, after the first action, is kept.
    let t3 = action(&mut fs, 3, |fs| fs.write_file(f, &mine).unwrap());
    action(&mut fs, 4, |fs| fs.write_file(f, &later).unwrap());
    let (out, _) = undo(&mut fs, t3);
    assert!(
        out.iter()
            .any(|(i, o)| *i == f && matches!(o, Outcome::Beside { .. })),
        "{out:?}"
    );
    assert_eq!(read(&mut fs, "/big.bin").unwrap(), later);
    checked(&mut fs);
}

/// A later change the person made is never lost: one that wrote other bytes,
/// or only renamed the file, still keeps the file as it is now.
#[test]
fn a_file_changed_later_than_the_action_left_it_is_still_kept() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let todo = write(&mut fs, "/todo.txt", b"orig");
    fs.commit().unwrap();
    let t1 = action(&mut fs, 1, |fs| fs.write_file(todo, b"a").unwrap());
    action(&mut fs, 2, |fs| {
        fs.rename(ROOT, "todo.txt", ROOT, "done.txt").unwrap();
    });
    undo(&mut fs, t1);
    assert_eq!(read(&mut fs, "/done.txt").unwrap(), b"a");
    assert_eq!(read(&mut fs, "/todo.txt (restored)").unwrap(), b"orig");
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

/// An object the action changed and a later change destroyed (an emptied
/// trash, which has no undo) stays gone: a restore of the action leaves it,
/// whether the emptying was an action or a change outside one (bug hunt 2,
/// H2-2).
#[test]
fn a_restore_leaves_what_a_later_change_destroyed() {
    for in_action in [true, false] {
        let mut fs = fresh();
        fs.set_volume(HOME).unwrap();
        let user = mkdir(&mut fs, "/user");
        let f = write(&mut fs, "/user/notes.txt", b"v1");
        fs.commit().unwrap();
        let t1 = action(&mut fs, 1, |fs| fs.write_file(f, b"v2").unwrap());
        let destroy = |fs: &mut Fs<MemDisk>| {
            fs.trash(user, "notes.txt").unwrap();
            fs.empty_trash().unwrap();
        };
        match in_action {
            true => {
                action(&mut fs, 2, destroy);
            }
            false => {
                destroy(&mut fs);
                fs.commit().unwrap();
            }
        }
        let (out, _) = undo(&mut fs, t1);
        assert!(
            out.contains(&(f, Outcome::Left { folder: false })),
            "{in_action}: {out:?}"
        );
        assert!(
            out.iter().all(|(_, o)| matches!(o, Outcome::Left { .. })),
            "{out:?}"
        );
        assert_eq!(fs.resolve("/user/notes.txt"), Err(Error::NotFound));
        assert!(fs.trash_list().unwrap().is_empty());
        checked(&mut fs);
    }
}

/// A remove brings a file back after later actions, as the tree its commit
/// left still shows it removed it; once that file is restored and removed
/// again by a later action, restoring the first action again leaves it.
#[test]
fn a_restore_brings_back_what_the_action_removed_and_only_that() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let f = write(&mut fs, "/a.txt", b"a");
    fs.commit().unwrap();
    let t1 = action(&mut fs, 1, |fs| fs.remove(ROOT, "a.txt").unwrap());
    action(&mut fs, 2, |fs| {
        write(fs, "/b.txt", b"b");
    });
    let (out, _) = undo(&mut fs, t1);
    assert!(out.contains(&(f, Outcome::Back)), "{out:?}");
    assert_eq!(read(&mut fs, "/a.txt").unwrap(), b"a");
    action(&mut fs, 3, |fs| fs.remove(ROOT, "a.txt").unwrap());
    let (out, _) = undo(&mut fs, t1);
    assert!(
        out.contains(&(f, Outcome::Left { folder: false })),
        "{out:?}"
    );
    assert_eq!(fs.resolve("/a.txt"), Err(Error::NotFound));
    checked(&mut fs);
}

/// A change outside any action leaves no record of what it removed, so once
/// one has landed in the volume, the tree the action's commit left is gone
/// and a removal can no longer be dated: the object is left rather than
/// brought back over what may have been a later removal for good.
#[test]
fn a_removal_no_tree_dates_is_left() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let f = write(&mut fs, "/a.txt", b"a");
    fs.commit().unwrap();
    let t1 = action(&mut fs, 1, |fs| fs.remove(ROOT, "a.txt").unwrap());
    write(&mut fs, "/b.txt", b"b");
    fs.commit().unwrap();
    let (out, _) = undo(&mut fs, t1);
    assert!(
        out.contains(&(f, Outcome::Left { folder: false })),
        "{out:?}"
    );
    assert_eq!(fs.resolve("/a.txt"), Err(Error::NotFound));
    checked(&mut fs);
}

/// An item a restore from the trash took out, then trashed again and emptied
/// outside any action, stays gone when that restore is undone: no tree dates
/// the trash folder's removal, so nothing destroyed later comes back (bug hunt
/// 2, H2-2; #1365's review).
#[test]
fn an_undo_of_a_restore_from_the_trash_leaves_what_was_emptied_since() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let user = mkdir(&mut fs, "/user");
    let f = write(&mut fs, "/user/notes.txt", b"v1");
    let held = fs.trash(user, "notes.txt").unwrap();
    fs.commit().unwrap();
    let folder = fs.path(None, held.folder).unwrap().unwrap();
    // The restore from the trash, as `fs` makes it: the item out, its folder
    // and the transaction's folder gone.
    let t1 = action(&mut fs, 1, |fs| {
        fs.rename(held.folder, &held.name, user, "notes.txt")
            .unwrap();
        fs.remove(held.folder, kaifs::trash::INFO).unwrap();
        let (slots, n) = split(&folder);
        let (trash, txn) = split(slots);
        let slots = fs.resolve(slots).unwrap();
        fs.remove(slots, n).unwrap();
        let trash = fs.resolve(trash).unwrap();
        fs.remove(trash, txn).unwrap();
    });
    // Trashed again and emptied, outside any action: gone for good.
    fs.trash(user, "notes.txt").unwrap();
    fs.empty_trash().unwrap();
    fs.commit().unwrap();
    let (out, _) = undo(&mut fs, t1);
    assert!(
        out.contains(&(f, Outcome::Left { folder: false })),
        "{out:?}"
    );
    assert_eq!(fs.resolve("/user/notes.txt"), Err(Error::NotFound));
    assert!(fs.trash_list().unwrap().is_empty(), "{out:?}");
    assert!(fs.stat(f).is_err(), "{out:?}");
    checked(&mut fs);
}

/// A scoped restore that would bring back what the action removed into the
/// volume's root, its folder gone since, is refused.
#[test]
fn a_scoped_restore_refuses_the_root() {
    let mut fs = fresh();
    fs.set_volume(HOME).unwrap();
    let pkg = mkdir(&mut fs, "/pkg");
    let x = mkdir(&mut fs, "/pkg/x");
    write(&mut fs, "/pkg/x/planted", b"v1");
    fs.commit().unwrap();
    let t1 = action(&mut fs, 1, |fs| fs.remove(x, "planted").unwrap());
    action(&mut fs, 2, |fs| fs.remove(pkg, "x").unwrap());
    fs.begin(3).unwrap();
    assert_eq!(fs.restore_within(t1, pkg).unwrap(), None);
    fs.abort().unwrap();
    // Unscoped, it lands in the root.
    let (out, _) = undo(&mut fs, t1);
    assert!(
        out.contains(&(x, Outcome::Left { folder: true })),
        "{out:?}"
    );
    assert_eq!(read(&mut fs, "/planted").unwrap(), b"v1");
    checked(&mut fs);
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
