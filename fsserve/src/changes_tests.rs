//! [`Request::ChangesSince`] (and [`Request::Changes`], its first page):
//! committed changes in `(changed, object)` order, a page at a time, so many
//! objects of one generation page through exactly once, a commit between two
//! pages is not missed, an open action's objects show once it commits, and a
//! folder's holder sees only what lies beneath it.

use super::tests::{ALICE, BLOCKS, BOB, Client};
use super::*;
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::vec;
use alloc::vec::Vec;
use kaifs::MemDisk;

fn on(c: &mut Client<'_>, caller: u64, badge: u64, request: Request) -> (Reply, Option<Mint>) {
    c.call(caller, badge, DIR_RIGHTS, request)
}

fn open(c: &mut Client<'_>, dir: u64, path: &str, how: How) -> Mint {
    let request = Request::Open {
        path: path.into(),
        rights: DIR_RIGHTS.bits(),
        how,
    };
    match on(c, ALICE, dir, request) {
        (Reply::Opened(..), Some(mint)) => mint,
        other => panic!("{path}: {other:?}"),
    }
}

fn object(mint: &Mint) -> u64 {
    Badge::parse(mint.badge).object
}

/// One page: its rows' objects, the cursor for the next (none at the end),
/// and its `upto` and `fallbacks`.
struct Page {
    objects: Vec<u64>,
    next: Option<(u64, u64)>,
    upto: u64,
    fallbacks: u64,
}

fn page(c: &mut Client<'_>, badge: u64, since: u64, after: Option<(u64, u64)>) -> Page {
    let request = Request::ChangesSince { since, after };
    let Reply::ChangesPage {
        rows,
        more,
        upto,
        fallbacks,
    } = on(c, BOB, badge, request).0
    else {
        panic!("not a page");
    };
    let pairs: Vec<(u64, u64)> = rows.iter().map(|r| (r.changed, r.object)).collect();
    assert!(pairs.is_sorted(), "a page out of order");
    assert!(pairs.iter().all(|&(changed, _)| changed <= upto));
    Page {
        objects: rows.iter().map(|r| r.object).collect(),
        next: pairs.last().copied().filter(|_| more),
        upto,
        fallbacks,
    }
}

/// Every page on `badge` since `since`: the objects in the order given, the
/// pages, and the last page's `upto`.
fn page_through(c: &mut Client<'_>, badge: u64, since: u64) -> (Vec<u64>, usize, u64) {
    let mut seen = Vec::new();
    let mut after = None;
    for pages in 1..1_000 {
        let got = page(c, badge, since, after);
        seen.extend(got.objects);
        match got.next {
            Some(next) => after = Some(next),
            None => return (seen, pages, got.upto),
        }
    }
    panic!("paging never ended");
}

fn commit(c: &mut Client<'_>) {
    let reply = on(c, ALICE, HOME_USER, Request::Commit).0;
    assert!(matches!(reply, Reply::Committed { .. }), "{reply:?}");
}

#[test]
fn two_hundred_objects_of_one_generation_page_through_exactly_once() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let (_, _, before) = page_through(&mut c, HOME_USER, 0);
    let begin = Request::Begin { action: 1 };
    assert_eq!(on(&mut c, ALICE, HOME_USER, begin).0, Reply::Done);
    let many = open(&mut c, HOME_USER, "many", How::Mkdir);
    let made: BTreeSet<u64> = (0..200)
        .map(|n| object(&open(&mut c, many.badge, &format!("f{n}"), How::CreateFile)))
        .collect();
    commit(&mut c);
    let (seen, pages, _) = page_through(&mut c, HOME_USER, before);
    let unique: BTreeSet<u64> = seen.iter().copied().collect();
    assert_eq!(unique.len(), seen.len(), "an object came twice");
    assert!(made.is_subset(&unique), "an object was skipped");
    assert!(pages >= 4, "200 rows in {pages} pages of {MAX_ROWS}");
}

/// A commit between two pages: an object already given comes again, later,
/// and a new one is not missed.
#[test]
fn a_commit_between_two_pages_is_not_missed() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let (_, _, before) = page_through(&mut c, HOME_USER, 0);
    let begin = Request::Begin { action: 1 };
    assert_eq!(on(&mut c, ALICE, HOME_USER, begin).0, Reply::Done);
    let files: Vec<Mint> = (0..100)
        .map(|n| open(&mut c, HOME_USER, &format!("f{n}"), How::CreateFile))
        .collect();
    commit(&mut c);
    let first = page(&mut c, HOME_USER, before, None);
    let given = first.objects[3];
    let cursor = first.next.expect("more than one page");
    // Between the pages: an object on the first page changes, one is made.
    let again = files.iter().find(|m| object(m) == given).expect("a file");
    let write = Request::Write {
        off: 0,
        data: vec![1; 10],
    };
    assert_eq!(
        c.call(ALICE, again.badge, again.rights, write).0,
        Reply::Done
    );
    let late = object(&open(&mut c, HOME_USER, "late", How::CreateFile));
    let mut rest = Vec::new();
    let mut after = Some(cursor);
    while let Some(at) = after {
        let got = page(&mut c, HOME_USER, before, Some(at));
        rest.extend(got.objects);
        after = got.next;
    }
    assert!(rest.contains(&given), "the changed object came again");
    assert!(rest.contains(&late), "the new object was missed");
    let all: BTreeSet<u64> = first.objects.iter().chain(&rest).copied().collect();
    for file in &files {
        assert!(all.contains(&object(file)), "a file was skipped");
    }
}

/// An open action's objects are not given until it commits, then they are,
/// past the `upto` of the pages before.
#[test]
fn an_open_actions_objects_show_once_it_commits() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let begin = Request::Begin { action: 1 };
    assert_eq!(on(&mut c, ALICE, HOME_USER, begin).0, Reply::Done);
    let pending = object(&open(&mut c, HOME_USER, "pending", How::CreateFile));
    let (seen, _, upto) = page_through(&mut c, HOME_USER, 0);
    assert!(!seen.contains(&pending), "an uncommitted object was given");
    commit(&mut c);
    let (seen, _, after) = page_through(&mut c, HOME_USER, upto);
    assert!(seen.contains(&pending));
    assert!(after > upto);
}

#[test]
fn a_folders_holder_sees_only_what_lies_beneath_it() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let a = open(&mut c, HOME_USER, "a", How::Mkdir);
    let b = open(&mut c, HOME_USER, "b", How::Mkdir);
    let x = open(&mut c, a.badge, "x", How::CreateFile);
    let y = open(&mut c, b.badge, "y", How::CreateFile);
    let (in_a, _, _) = page_through(&mut c, a.badge, 0);
    assert!(in_a.contains(&object(&x)) && in_a.contains(&object(&a)));
    assert!(!in_a.contains(&object(&y)) && !in_a.contains(&object(&b)));
    // `/home/user`'s holder sees both folders.
    let (in_home, _, _) = page_through(&mut c, HOME_USER, 0);
    for mint in [&a, &b, &x, &y] {
        assert!(in_home.contains(&object(mint)));
    }
}

/// An object moved out of a folder, or into the trash, is no longer the
/// folder's: its holder no longer sees it, and the new folder's does.
#[test]
fn an_object_moved_out_or_trashed_leaves_its_folders_changes() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let a = open(&mut c, HOME_USER, "a", How::Mkdir);
    let b = open(&mut c, HOME_USER, "b", How::Mkdir);
    let moved = object(&open(&mut c, a.badge, "moved", How::CreateFile));
    let trashed = object(&open(&mut c, a.badge, "trashed", How::CreateFile));
    let (in_a, _, _) = page_through(&mut c, a.badge, 0);
    assert!(in_a.contains(&moved) && in_a.contains(&trashed));
    // Moved by kaifs itself: `kai-fs 1`'s Rename stays in one folder.
    let fs = c.server.fs();
    fs.set_volume(kaifs::vol::HOME).unwrap();
    fs.rename(object(&a), "moved", object(&b), "moved").unwrap();
    fs.commit().unwrap();
    let trash = Request::Trash {
        name: "trashed".into(),
    };
    assert_eq!(on(&mut c, ALICE, a.badge, trash).0, Reply::Done);
    let (in_a, _, _) = page_through(&mut c, a.badge, 0);
    assert!(!in_a.contains(&moved) && !in_a.contains(&trashed));
    let (in_b, _, _) = page_through(&mut c, b.badge, 0);
    assert!(in_b.contains(&moved));
}

#[test]
fn a_page_says_how_many_mounts_fell_back() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    assert_eq!(page(&mut c, HOME_USER, 0, None).fallbacks, 0);
}
