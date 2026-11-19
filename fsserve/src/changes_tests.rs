//! [`Request::Changes`] and [`Request::ChangesAfter`]: pages in
//! `(changed, object)` order, so many objects of one generation page through
//! exactly once, and a folder's holder sees only what lies beneath it.

use super::tests::{ALICE, BLOCKS, Client};
use super::*;
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::vec::Vec;
use kaifs::MemDisk;

fn on(c: &mut Client<'_>, badge: u64, request: Request) -> (Reply, Option<Mint>) {
    c.call(ALICE, badge, DIR_RIGHTS, request)
}

fn open(c: &mut Client<'_>, dir: u64, path: &str, how: How) -> Mint {
    let request = Request::Open {
        path: path.into(),
        rights: DIR_RIGHTS.bits(),
        how,
    };
    match on(c, dir, request) {
        (Reply::Opened(..), Some(mint)) => mint,
        other => panic!("{path}: {other:?}"),
    }
}

fn object(mint: &Mint) -> u64 {
    Badge::parse(mint.badge).object
}

/// Every page of the changes on `badge` since `since`: the objects in the
/// order given, and how many pages.
fn page_through(c: &mut Client<'_>, badge: u64, since: u64) -> (Vec<u64>, usize) {
    let mut seen = Vec::new();
    let mut request = Request::Changes { since };
    for pages in 1..1_000 {
        let Reply::Changes { rows, more } = on(c, badge, request).0 else {
            panic!("not changes");
        };
        let pairs: Vec<(u64, u64)> = rows.iter().map(|r| (r.changed, r.object)).collect();
        assert!(pairs.is_sorted(), "a page out of order");
        seen.extend(rows.iter().map(|r| r.object));
        let Some(&after) = pairs.last().filter(|_| more) else {
            return (seen, pages);
        };
        request = Request::ChangesAfter { since, after };
    }
    panic!("paging never ended");
}

#[test]
fn two_hundred_objects_of_one_generation_page_through_exactly_once() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    assert_eq!(
        on(&mut c, HOME_USER, Request::Begin { action: 1 }).0,
        Reply::Done
    );
    let Reply::Generation { generation: before } = on(&mut c, HOME_USER, Request::Generation).0
    else {
        panic!("no generation");
    };
    let many = open(&mut c, HOME_USER, "many", How::Mkdir);
    let made: BTreeSet<u64> = (0..200)
        .map(|n| object(&open(&mut c, many.badge, &format!("f{n}"), How::CreateFile)))
        .collect();
    assert!(matches!(
        on(&mut c, HOME_USER, Request::Commit).0,
        Reply::Committed { .. }
    ));
    let (seen, pages) = page_through(&mut c, HOME_USER, before - 1);
    let unique: BTreeSet<u64> = seen.iter().copied().collect();
    assert_eq!(unique.len(), seen.len(), "an object came twice");
    assert!(made.is_subset(&unique), "an object was skipped");
    assert!(pages >= 4, "200 rows in {pages} pages of {MAX_ROWS}");
}

#[test]
fn a_folders_holder_sees_only_what_lies_beneath_it() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let a = open(&mut c, HOME_USER, "a", How::Mkdir);
    let b = open(&mut c, HOME_USER, "b", How::Mkdir);
    let x = open(&mut c, a.badge, "x", How::CreateFile);
    let y = open(&mut c, b.badge, "y", How::CreateFile);
    let (in_a, _) = page_through(&mut c, a.badge, 0);
    let in_a: BTreeSet<u64> = in_a.into_iter().collect();
    assert!(in_a.contains(&object(&x)) && in_a.contains(&object(&a)));
    assert!(!in_a.contains(&object(&y)) && !in_a.contains(&object(&b)));
    // `/home/user`'s holder sees both folders.
    let (in_home, _) = page_through(&mut c, HOME_USER, 0);
    for mint in [&a, &b, &x, &y] {
        assert!(in_home.contains(&object(mint)));
    }
}
