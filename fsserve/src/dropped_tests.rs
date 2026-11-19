//! An action dropped without its owner asking: its pending run fails to write
//! out on another caller's request, on a full disk. The owner's later changes
//! are refused [`Error::Dropped`], never committed one by one outside any
//! action, until it sends Abort or Begin.

use super::tests::{ALICE, BOB, Client};
use super::*;
use alloc::format;
use alloc::vec;
use kaifs::MemDisk;

fn home(c: &mut Client<'_>, caller: u64, request: Request) -> (Reply, Option<Mint>) {
    c.call(caller, HOME_USER, DIR_RIGHTS, request)
}

fn create(c: &mut Client<'_>, name: &str) -> Option<Mint> {
    let open = Request::Open {
        path: name.into(),
        rights: DIR_RIGHTS.bits(),
        how: How::CreateFile,
    };
    home(c, ALICE, open).1
}

fn write(c: &mut Client<'_>, file: &Mint, off: u64, len: usize) -> Reply {
    let request = Request::Write {
        off,
        data: vec![7; len],
    };
    c.call(ALICE, file.badge, file.rights, request).0
}

/// Fills the disk with 4 KiB files until a write is refused.
fn fill(c: &mut Client<'_>) {
    for n in 0..10_000 {
        let Some(file) = create(c, &format!("fill{n}")) else {
            return;
        };
        if write(c, &file, 0, 4096) != Reply::Done {
            return;
        }
    }
    panic!("the disk never filled");
}

#[test]
fn a_dropped_owner_is_refused_until_it_aborts() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let doc = create(&mut c, "doc").expect("a file");
    assert_eq!(write(&mut c, &doc, 0, 100), Reply::Done);
    fill(&mut c);
    // Alice's action: a run of 100 KiB waits in `fs`, short of a record.
    assert_eq!(
        home(&mut c, ALICE, Request::Begin { action: 1 }).0,
        Reply::Done
    );
    assert_eq!(write(&mut c, &doc, 0, 100 << 10), Reply::Done);
    // Bob's read writes the run out first; there is no room, so Alice's
    // action is dropped and Bob is told.
    let bob = home(&mut c, BOB, Request::Stat).0;
    assert!(matches!(bob, Reply::Failed(_)), "{bob:?}");
    // Alice's next write is refused, not committed on its own.
    assert_eq!(write(&mut c, &doc, 200, 10), Reply::Failed(Error::Dropped));
    assert_eq!(
        home(&mut c, ALICE, Request::Commit).0,
        Reply::Failed(Error::Dropped)
    );
    // Her Abort is answered Done, and then her changes land again.
    assert_eq!(home(&mut c, ALICE, Request::Abort).0, Reply::Done);
    let Reply::Data(bytes) = c
        .call(
            ALICE,
            doc.badge,
            doc.rights,
            Request::Read { off: 0, len: 4096 },
        )
        .0
    else {
        panic!("no data");
    };
    assert_eq!(bytes, vec![7; 100], "none of the dropped action landed");
    // Not refused any more (on this full disk it may still find no room).
    assert_ne!(write(&mut c, &doc, 0, 10), Reply::Failed(Error::Dropped));
}

#[test]
fn a_begin_also_ends_the_refusal_and_bob_is_never_refused() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let doc = create(&mut c, "doc").expect("a file");
    fill(&mut c);
    assert_eq!(
        home(&mut c, ALICE, Request::Begin { action: 1 }).0,
        Reply::Done
    );
    assert_eq!(write(&mut c, &doc, 0, 100 << 10), Reply::Done);
    let _ = home(&mut c, BOB, Request::Stat);
    // Bob's own changes are not refused: the action was Alice's.
    let trash = Request::Trash {
        name: "fill0".into(),
    };
    assert_ne!(home(&mut c, BOB, trash).0, Reply::Failed(Error::Dropped));
    assert_eq!(
        home(&mut c, ALICE, Request::Begin { action: 2 }).0,
        Reply::Done
    );
    assert_eq!(write(&mut c, &doc, 0, 10), Reply::Done);
    assert_eq!(home(&mut c, ALICE, Request::Abort).0, Reply::Done);
}
