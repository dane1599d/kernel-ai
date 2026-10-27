//! The namespace's behaviour, one case at a time (P2 task 5): names, folders,
//! whole-file and ranged writes, holes, rename, remove, attributes, colliding
//! name hashes, inode numbers that are never reused, the rules `check` holds
//! them to, and `check --repair`'s rules on namespaces broken on purpose.

use kaifs::check::{Cache, check};
use kaifs::format::{self, Superblock};
use kaifs::fs::SYSTEM;
use kaifs::fs::inline_of;
use kaifs::items::{Entry, K_DATA, K_DIRENT, K_INODE, K_XATTR, encode_names};
use kaifs::ns::Listed;
use kaifs::ns_check::check_namespace;
use kaifs::repair::repair;
use kaifs::tree::{Child, Node};
use kaifs::{BLOCK, BlockDev, Error, FileType, Fs, Key, MemDisk, ROOT};

const BLOCKS: u64 = 8192;

fn fresh() -> Fs<MemDisk> {
    Fs::mkfs(MemDisk::new(BLOCKS), [9; 16], "namespace").unwrap()
}

fn remount(fs: Fs<MemDisk>) -> Fs<MemDisk> {
    Fs::mount(fs.into_dev()).unwrap()
}

/// Commits, then holds the block and namespace rules; returns what the
/// namespace check found on the live tree.
fn commit_and_check(fs: &mut Fs<MemDisk>) -> kaifs::ns_check::NsReport {
    fs.commit().unwrap();
    let mut cache = Cache::default();
    check(fs, &mut cache).unwrap();
    (*check_namespace(fs, &mut cache).unwrap().volumes[&SYSTEM].live).clone()
}

fn bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn names(fs: &mut Fs<MemDisk>, dir: u64) -> Vec<String> {
    let mut out: Vec<String> = fs
        .list(dir, None, usize::MAX)
        .unwrap()
        .into_iter()
        .map(|l| l.entry.name)
        .collect();
    out.sort();
    out
}

#[test]
fn mkfs_makes_an_empty_root_that_checks_clean() {
    let mut fs = remount(fresh());
    let root = fs.stat(ROOT).unwrap();
    assert_eq!(
        (root.kind, root.nlink, root.size, root.parent),
        (FileType::Dir, 2, 0, ROOT)
    );
    assert_eq!(fs.resolve("/").unwrap(), ROOT);
    assert!(names(&mut fs, ROOT).is_empty());
    let mut cache = Cache::default();
    check(&mut fs, &mut cache).unwrap();
    let ns = check_namespace(&mut fs, &mut cache).unwrap();
    let live = &ns.volumes[&SYSTEM].live;
    assert_eq!((live.dirs, live.files), (1, 0));
    assert_eq!(ns.volumes.len(), 3, "system, home and models");
}

#[test]
fn names_folders_and_lookups() {
    let mut fs = fresh();
    let docs = fs.mkdir(ROOT, "docs").unwrap();
    let note = fs.create(docs, "note.txt").unwrap();
    fs.create(ROOT, "readme").unwrap();
    assert_eq!(fs.create(docs, "note.txt"), Err(Error::Exists));
    assert_eq!(fs.mkdir(ROOT, "readme"), Err(Error::Exists));
    assert_eq!(fs.create(note, "x"), Err(Error::NotDir));
    for bad in ["", ".", "..", "a/b", "nul\0", &"n".repeat(256)] {
        assert_eq!(
            fs.create(ROOT, bad),
            Err(Error::Invalid("a name")),
            "{bad:?}"
        );
    }
    fs.create(ROOT, &"n".repeat(255)).unwrap();
    fs.create(ROOT, "日本語の名前").unwrap();
    assert_eq!(fs.resolve("/docs/note.txt").unwrap(), note);
    assert_eq!(fs.resolve("/docs/none"), Err(Error::NotFound));
    assert_eq!(fs.resolve("docs"), Err(Error::Invalid("a path not from /")));
    assert_eq!(
        fs.resolve("/docs/../docs"),
        Err(Error::Invalid("a path component"))
    );
    let report = commit_and_check(&mut fs);
    assert_eq!((report.dirs, report.files), (2, 4));
    let mut fs = remount(fs);
    let root = fs.stat(ROOT).unwrap();
    assert_eq!((root.size, root.nlink), (4, 3));
    assert_eq!(fs.lookup(docs, "note.txt").unwrap().unwrap().ino, note);
    assert_eq!(fs.stat(note).unwrap().parent, docs);
}

#[test]
fn a_listing_pages_through_a_cursor() {
    let mut fs = fresh();
    for i in 0..300 {
        fs.create(ROOT, &format!("file-{i:03}")).unwrap();
    }
    let mut seen = Vec::new();
    let mut after: Option<Listed> = None;
    loop {
        let page = fs.list(ROOT, after.as_ref(), 7).unwrap();
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 7);
        seen.extend(page.iter().map(|l| l.entry.name.clone()));
        after = page.last().cloned();
    }
    assert_eq!(seen.len(), 300);
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 300);
    commit_and_check(&mut fs);
}

#[test]
fn whole_file_writes_inline_in_a_record_and_across_records() {
    let mut fs = fresh();
    let f = fs.create(ROOT, "f").unwrap();
    for (seed, len) in [
        (1, 0),
        (2, 10),
        (3, 1023),
        (4, 1024),
        (5, 5000),
        (6, 128 << 10),
        (7, 300_000),
        (8, 3),
    ] {
        let data = bytes(seed, len);
        fs.write_file(f, &data).unwrap();
        assert_eq!(fs.read_file(f).unwrap(), data, "before commit, {len} bytes");
        let report = commit_and_check(&mut fs);
        assert_eq!(report.bytes, len as u64);
        let mut again = remount(core::mem::replace(&mut fs, fresh()));
        assert_eq!(
            again.read_file(f).unwrap(),
            data,
            "after a remount, {len} bytes"
        );
        fs = again;
    }
}

#[test]
fn ranged_writes_holes_and_truncation() {
    let mut fs = fresh();
    let f = fs.create(ROOT, "sparse").unwrap();
    let mut model = vec![0u8; 0];
    let write = |fs: &mut Fs<MemDisk>, model: &mut Vec<u8>, off: usize, data: &[u8]| {
        fs.write_at(f, off as u64, data).unwrap();
        if model.len() < off + data.len() {
            model.resize(off + data.len(), 0);
        }
        model[off..off + data.len()].copy_from_slice(data);
    };
    write(&mut fs, &mut model, 0, b"head");
    write(&mut fs, &mut model, 300_000, &bytes(1, 5000));
    write(&mut fs, &mut model, 130_000, &bytes(2, 2000));
    write(&mut fs, &mut model, 131_000, &bytes(3, 20));
    assert_eq!(fs.read_file(f).unwrap(), model);
    commit_and_check(&mut fs);
    assert_eq!(fs.read_at(f, 299_990, 20).unwrap(), model[299_990..300_010]);
    assert_eq!(fs.read_at(f, 1 << 40, 10).unwrap(), Vec::<u8>::new());
    for size in [302_000usize, 140_000, 131_010, 131_072, 4, 0, 1 << 20] {
        fs.truncate(f, size as u64).unwrap();
        model.resize(size, 0);
        assert_eq!(fs.read_file(f).unwrap(), model, "truncated to {size}");
        assert_eq!(commit_and_check(&mut fs).bytes, size as u64);
    }
    assert!(matches!(
        fs.write_at(f, 1 << 48, b"x"),
        Err(Error::Invalid(_))
    ));
    let d = fs.mkdir(ROOT, "d").unwrap();
    assert_eq!(fs.write_file(d, b"x"), Err(Error::IsDir));
}

#[test]
fn rename_within_and_across_folders() {
    let mut fs = fresh();
    let a = fs.mkdir(ROOT, "a").unwrap();
    let b = fs.mkdir(ROOT, "b").unwrap();
    let inner = fs.mkdir(a, "inner").unwrap();
    let f = fs.create(a, "f").unwrap();
    fs.write_file(f, b"payload").unwrap();
    fs.rename(a, "f", a, "g").unwrap();
    fs.rename(a, "g", b, "h").unwrap();
    fs.rename(a, "inner", b, "inner").unwrap();
    fs.rename(b, "h", b, "h").unwrap();
    assert_eq!(fs.rename(b, "none", a, "x"), Err(Error::NotFound));
    fs.create(a, "taken").unwrap();
    assert_eq!(fs.rename(b, "h", a, "taken"), Err(Error::Exists));
    assert_eq!(
        fs.rename(ROOT, "b", b, "b2"),
        Err(Error::Invalid("a folder cannot move below itself"))
    );
    assert_eq!(
        fs.rename(ROOT, "b", inner, "b2"),
        Err(Error::Invalid("a folder cannot move below itself"))
    );
    commit_and_check(&mut fs);
    assert_eq!(fs.resolve("/b/h").unwrap(), f);
    assert_eq!(fs.read_file(f).unwrap(), b"payload");
    assert_eq!(fs.stat(f).unwrap().parent, b);
    assert_eq!(fs.stat(inner).unwrap().parent, b);
    assert_eq!(
        (fs.stat(a).unwrap().nlink, fs.stat(b).unwrap().nlink),
        (2, 3)
    );
    assert_eq!(names(&mut fs, a), ["taken"]);
    assert_eq!(names(&mut fs, b), ["h", "inner"]);
}

#[test]
fn remove_frees_a_file_and_refuses_a_full_folder() {
    let mut fs = fresh();
    let d = fs.mkdir(ROOT, "d").unwrap();
    let f = fs.create(d, "big").unwrap();
    fs.write_file(f, &bytes(5, 600_000)).unwrap();
    fs.set_xattr(f, "user.kind", b"blob").unwrap();
    commit_and_check(&mut fs);
    assert_eq!(fs.remove(ROOT, "d"), Err(Error::NotEmpty));
    fs.remove(d, "big").unwrap();
    assert_eq!(fs.stat(f), Err(Error::NotFound));
    assert_eq!(fs.remove(d, "big"), Err(Error::NotFound));
    fs.remove(ROOT, "d").unwrap();
    let mut items = 0;
    fs.scan_raw(&Key::MIN, &mut |_, _| {
        items += 1;
        true
    })
    .unwrap();
    assert_eq!(items, 1, "only the root's inode is left");
    commit_and_check(&mut fs);
    fs.commit().unwrap();
    fs.commit().unwrap();
    let report = check(&mut fs, &mut Cache::default()).unwrap();
    assert_eq!(report.leaked_blocks, 0, "the file's records were freed");
}

#[test]
fn extended_attributes() {
    let mut fs = fresh();
    let f = fs.create(ROOT, "f").unwrap();
    fs.set_xattr(f, "user.a", b"1").unwrap();
    fs.set_xattr(f, "user.b", b"").unwrap();
    fs.set_xattr(f, "user.a", b"one").unwrap();
    fs.set_xattr(ROOT, "user.root", b"r").unwrap();
    assert_eq!(fs.get_xattr(f, "user.a").unwrap(), Some(b"one".to_vec()));
    assert_eq!(fs.get_xattr(f, "user.none").unwrap(), None);
    assert!(matches!(
        fs.set_xattr(f, "big", &[0; 1100]),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(fs.set_xattr(f, "", b"x"), Err(Error::Invalid(_))));
    commit_and_check(&mut fs);
    let mut fs = remount(fs);
    let mut all = fs.xattrs(f).unwrap();
    all.sort();
    assert_eq!(
        all,
        [
            ("user.a".into(), b"one".to_vec()),
            ("user.b".into(), Vec::new())
        ]
    );
    assert!(fs.remove_xattr(f, "user.a").unwrap());
    assert!(!fs.remove_xattr(f, "user.a").unwrap());
    commit_and_check(&mut fs);
}

#[test]
fn colliding_name_hashes_share_an_item() {
    let mut fs = fresh();
    fs.hash_mask = 0x3;
    let d = fs.mkdir(ROOT, "d").unwrap();
    let mut made = Vec::new();
    for i in 0..60 {
        let name = format!("n{i}");
        made.push((name.clone(), fs.create(d, &name).unwrap()));
    }
    fs.set_xattr(d, "user.x", b"1").unwrap();
    fs.set_xattr(d, "user.y", b"2").unwrap();
    commit_and_check(&mut fs);
    let mut buckets = 0;
    fs.scan_raw(&Key::new(d, K_DIRENT, 0), &mut |k, _| {
        let mine = k.oid == d && k.kind == K_DIRENT;
        buckets += usize::from(mine);
        mine
    })
    .unwrap();
    assert!(buckets <= 4);
    for (name, ino) in &made {
        assert_eq!(fs.lookup(d, name).unwrap().unwrap().ino, *ino);
    }
    fs.rename(d, "n1", d, "renamed").unwrap();
    for i in (0..60).step_by(3) {
        if i != 1 {
            fs.remove(d, &format!("n{i}")).unwrap();
        }
    }
    commit_and_check(&mut fs);
    let mut paged = Vec::new();
    let mut after: Option<Listed> = None;
    while let Some(last) = {
        let page = fs.list(d, after.as_ref(), 3).unwrap();
        paged.extend(page.iter().map(|l| l.entry.name.clone()));
        page.last().cloned()
    } {
        after = Some(last);
    }
    paged.sort();
    assert_eq!(paged, names(&mut fs, d));
    assert_eq!(paged.len(), 60 - 20);
    // A chain is one item: long names fill it, and the next is refused.
    let e = fs.mkdir(ROOT, "e").unwrap();
    fs.hash_mask = 0;
    let long = |i: usize| format!("{i:0>250}");
    for i in 0..3 {
        fs.create(e, &long(i)).unwrap();
    }
    assert!(matches!(fs.create(e, &long(3)), Err(Error::Invalid(_))));
    assert_eq!(
        fs.stat(e).unwrap().size,
        3,
        "the refused create changed nothing"
    );
}

#[test]
fn inode_numbers_are_never_reused() {
    let mut fs = fresh();
    let a = fs.create(ROOT, "a").unwrap();
    fs.remove(ROOT, "a").unwrap();
    let b = fs.create(ROOT, "b").unwrap();
    assert!(b > a);
    let snap = fs.snapshot().unwrap();
    commit_and_check(&mut fs);
    let c = fs.create(ROOT, "c").unwrap();
    fs.commit().unwrap();
    assert!(fs.restore_snapshot(snap).unwrap());
    let d = fs.create(ROOT, "d").unwrap();
    assert!(d > c, "a restore does not wind the counter back");
    commit_and_check(&mut fs);
    let mut fs = remount(fs);
    assert!(fs.create(ROOT, "e").unwrap() > d);
}

#[test]
fn a_read_only_filesystem_refuses_changes() {
    let mut disk = fresh().into_dev();
    let found = format::read_rings(&mut disk).unwrap();
    let mut sb: Superblock = found[0];
    sb.ro_compat = 1 << 7;
    sb.generation += 1;
    format::write_rings(&mut disk, &sb).unwrap();
    let mut fs = Fs::mount(disk).unwrap();
    assert_eq!(fs.create(ROOT, "x"), Err(Error::ReadOnly));
    assert_eq!(fs.mkdir(ROOT, "x"), Err(Error::ReadOnly));
    assert_eq!(fs.set_xattr(ROOT, "x", b""), Err(Error::ReadOnly));
    assert_eq!(repair(&mut fs), Err(Error::ReadOnly));
}

#[test]
fn repair_refuses_an_unknown_compat_feature() {
    let mut disk = fresh().into_dev();
    let found = format::read_rings(&mut disk).unwrap();
    let mut sb: Superblock = found[0];
    sb.compat = 1 << 9;
    sb.generation += 1;
    format::write_rings(&mut disk, &sb).unwrap();
    let mut fs = Fs::mount(disk).unwrap();
    assert!(fs.writable());
    assert!(matches!(repair(&mut fs), Err(Error::Unsupported(_))));
}

/// A namespace with something of everything, committed.
fn populated() -> (Fs<MemDisk>, u64, u64, u64) {
    let mut fs = fresh();
    let docs = fs.mkdir(ROOT, "docs").unwrap();
    let deep = fs.mkdir(docs, "deep").unwrap();
    let f = fs.create(docs, "report").unwrap();
    fs.write_file(f, &bytes(1, 200_000)).unwrap();
    let g = fs.create(deep, "small").unwrap();
    fs.write_file(g, b"tiny").unwrap();
    fs.set_xattr(g, "user.tag", b"t").unwrap();
    for i in 0..150 {
        let h = fs.create(ROOT, &format!("filler-{i}")).unwrap();
        fs.write_file(h, &bytes(i, 40)).unwrap();
    }
    commit_and_check(&mut fs);
    (fs, docs, deep, f)
}

/// Breaks `fs` with `damage`, commits; check must fail as `why` says, repair must
/// fix it, and check must then pass. Returns the repair's lines.
fn broken(damage: impl FnOnce(&mut Fs<MemDisk>), why: &str) -> (Fs<MemDisk>, Vec<String>) {
    let (mut fs, ..) = populated();
    damage(&mut fs);
    fs.commit().unwrap();
    let mut fs = remount(fs);
    let mut cache = Cache::default();
    let found =
        check(&mut fs, &mut cache).and_then(|_| check_namespace(&mut fs, &mut cache).map(|_| ()));
    let err = found.expect_err("the damage went unnoticed");
    assert!(err.contains(why), "{err:?} does not say {why:?}");
    let fixes = repair(&mut fs).unwrap();
    assert!(!fixes.is_empty());
    let mut fs = remount(fs);
    let mut cache = Cache::default();
    check(&mut fs, &mut cache).unwrap();
    check_namespace(&mut fs, &mut cache).unwrap_or_else(|e| panic!("{e} after {fixes:#?}"));
    assert!(
        repair(&mut fs).unwrap().is_empty(),
        "a second repair finds nothing"
    );
    (fs, fixes)
}

#[test]
fn repair_leaves_a_sound_namespace_alone() {
    let (mut fs, ..) = populated();
    let generation = fs.committed;
    assert!(repair(&mut fs).unwrap().is_empty());
    assert_eq!(fs.committed, generation);
}

#[test]
fn repair_sets_wrong_counts_from_the_names() {
    let (mut fs, fixes) = broken(
        |fs| {
            let docs = fs.resolve("/docs").unwrap();
            let mut inode = fs.stat(docs).unwrap();
            inode.nlink = 9;
            fs.put(Key::new(docs, K_INODE, 0), &inode.encode()).unwrap();
        },
        "a folder's links",
    );
    assert!(fixes.iter().any(|l| l.contains("links")), "{fixes:?}");
    let docs = fs.resolve("/docs").unwrap();
    assert_eq!(fs.stat(docs).unwrap().nlink, 3);
}

#[test]
fn repair_drops_a_name_whose_inode_is_gone() {
    let (mut fs, fixes) = broken(
        |fs| {
            let small = fs.resolve("/docs/deep/small").unwrap();
            fs.delete(&Key::new(small, K_INODE, 0)).unwrap();
        },
        "items, but no inode",
    );
    assert!(
        fixes.iter().any(|l| l.contains("names no such inode")),
        "{fixes:?}"
    );
    assert_eq!(fs.resolve("/docs/deep/small"), Err(Error::NotFound));
    let report = fs.resolve("/docs/report").unwrap();
    assert_eq!(fs.read_file(report).unwrap(), bytes(1, 200_000));
}

#[test]
fn repair_names_an_orphan_in_lost_and_found() {
    let (mut fs, fixes) = broken(
        |fs| {
            // The name goes, and the folder's counts with it: only the orphan
            // is wrong.
            let docs = fs.resolve("/docs").unwrap();
            let hash = fs.name_hash("deep");
            fs.delete(&Key::new(docs, K_DIRENT, hash)).unwrap();
            let mut inode = fs.stat(docs).unwrap();
            (inode.size, inode.nlink) = (inode.size - 1, inode.nlink - 1);
            fs.put(Key::new(docs, K_INODE, 0), &inode.encode()).unwrap();
        },
        "a folder named other than once",
    );
    assert!(fixes.iter().any(|l| l.contains("/lost+found")), "{fixes:?}");
    let lost = fs.resolve("/lost+found").unwrap();
    let names = names(&mut fs, lost);
    assert_eq!(names.len(), 1);
    let small = fs
        .resolve(&format!("/lost+found/{}/small", names[0]))
        .unwrap();
    assert_eq!(fs.read_file(small).unwrap(), b"tiny");
    assert_eq!(
        fs.get_xattr(small, "user.tag").unwrap(),
        Some(b"t".to_vec())
    );
}

#[test]
fn repair_breaks_a_folder_cycle_cut_off_from_the_root() {
    let (mut fs, fixes) = broken(
        |fs| {
            // /x/y, then x's name moves from the root into y: each folder is
            // named once, by its parent, and neither is reached.
            let x = fs.mkdir(ROOT, "x").unwrap();
            let y = fs.mkdir(x, "y").unwrap();
            let hash = fs.name_hash("x");
            fs.delete(&Key::new(ROOT, K_DIRENT, hash)).unwrap();
            let mut root = fs.stat(ROOT).unwrap();
            (root.size, root.nlink) = (root.size - 1, root.nlink - 1);
            fs.put(Key::new(ROOT, K_INODE, 0), &root.encode()).unwrap();
            let entry = Entry {
                name: "x".into(),
                ino: x,
                kind: FileType::Dir,
            };
            fs.put(Key::new(y, K_DIRENT, hash), &encode_names(&[entry]))
                .unwrap();
            let mut inode = fs.stat(y).unwrap();
            (inode.size, inode.nlink) = (1, 3);
            fs.put(Key::new(y, K_INODE, 0), &inode.encode()).unwrap();
            let mut inode = fs.stat(x).unwrap();
            inode.parent = y;
            fs.put(Key::new(x, K_INODE, 0), &inode.encode()).unwrap();
        },
        "not reached from the root",
    );
    assert!(
        fixes.iter().any(|l| l.contains("a second name")),
        "{fixes:?}"
    );
    let lost = fs.resolve("/lost+found").unwrap();
    let names = names(&mut fs, lost);
    assert_eq!(names.len(), 1);
    let y = fs.resolve(&format!("/lost+found/{}/y", names[0])).unwrap();
    assert_eq!(fs.stat(y).unwrap().size, 0, "y no longer names x");
}

#[test]
fn repair_files_a_name_again_under_its_hash() {
    let (mut fs, fixes) = broken(
        |fs| {
            let hash = fs.name_hash("docs");
            let raw = fs
                .get_raw(&Key::new(ROOT, K_DIRENT, hash))
                .unwrap()
                .unwrap();
            fs.delete(&Key::new(ROOT, K_DIRENT, hash)).unwrap();
            fs.put(Key::new(ROOT, K_DIRENT, hash ^ 1), inline_of(&raw).unwrap())
                .unwrap();
        },
        "filed under another hash",
    );
    assert!(fixes.iter().any(|l| l.contains("filed again")), "{fixes:?}");
    assert!(fs.resolve("/docs/deep/small").is_ok());
}

#[test]
fn repair_drops_items_it_cannot_place() {
    let (_, fixes) = broken(
        |fs| {
            let report = fs.resolve("/docs/report").unwrap();
            fs.put(Key::new(report, 9, 0), b"unknown kind").unwrap();
            fs.put(Key::new(report, K_DIRENT, 5), b"names in a file")
                .unwrap();
        },
        "names in a file",
    );
    assert!(
        fixes.iter().filter(|l| l.contains("dropped")).count() >= 2,
        "{fixes:?}"
    );
}

#[test]
fn repair_makes_misplaced_data_canonical() {
    let (mut fs, fixes) = broken(
        |fs| {
            let small = fs.resolve("/docs/deep/small").unwrap();
            // Four bytes in a record, where they belong inline.
            fs.put_record(Key::new(small, K_DATA, 0), b"tiny").unwrap();
            fs.put(Key::new(small, K_XATTR, 1), b"\x07garbage").unwrap();
        },
        "a record that would",
    );
    assert!(fixes.iter().any(|l| l.contains("made inline")), "{fixes:?}");
    let small = fs.resolve("/docs/deep/small").unwrap();
    assert_eq!(fs.read_file(small).unwrap(), b"tiny");
}

#[test]
fn repair_salvages_a_tree_with_a_node_that_no_longer_verifies() {
    let (fs, ..) = populated();
    let mut fs = remount(fs);
    let root = fs.live_root().unwrap();
    let Node::Branch(_, children) = fs.io.load_node(&root).unwrap() else {
        panic!("the populated tree has more than one leaf");
    };
    let Child::Clean(victim) = children[children.len() - 1].1 else {
        unreachable!("read from disk")
    };
    fs.io.dev.write(victim.addr, &[0xEE; BLOCK]).unwrap();
    let mut cache = Cache::default();
    assert!(check(&mut fs, &mut cache).is_err());
    let fixes = repair(&mut fs).unwrap();
    assert!(
        fixes.iter().any(|l| l.contains("does not verify")),
        "{fixes:?}"
    );
    let mut fs = remount(fs);
    let mut cache = Cache::default();
    check(&mut fs, &mut cache).unwrap();
    check_namespace(&mut fs, &mut cache).unwrap();
    assert_eq!(fs.stat(ROOT).unwrap().kind, FileType::Dir);
}
