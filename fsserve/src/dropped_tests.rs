//! An action dropped without its owner asking: its pending run fails to write
//! out on another caller's request, on a full disk. The owner's later changes
//! are refused [`Error::Dropped`], never committed one by one outside any
//! action, until it sends Abort or Begin.

use super::tests::{ALICE, BOB, Client};
use super::*;
use alloc::format;
use alloc::vec;
use kaifs::MemDisk;

/// A third caller.
const CAROL: u64 = 9;

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

/// Writes `len` bytes at `off` in `kai-fs 1`'s message pieces: the first
/// reply that is not Done, else Done.
fn write_run(c: &mut Client<'_>, file: &Mint, off: u64, len: usize) -> Reply {
    let mut at = 0;
    while at < len {
        let piece = (len - at).min(fsproto::MAX_DATA);
        let reply = write(c, file, off + at as u64, piece);
        if reply != Reply::Done {
            return reply;
        }
        at += piece;
    }
    Reply::Done
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
    assert_eq!(write_run(&mut c, &doc, 0, 100 << 10), Reply::Done);
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
    assert_eq!(write_run(&mut c, &doc, 0, 100 << 10), Reply::Done);
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

fn write_as(c: &mut Client<'_>, caller: u64, file: &Mint, len: usize) -> Reply {
    let request = Request::Write {
        off: 0,
        data: vec![3; len],
    };
    c.call(caller, file.badge, file.rights, request).0
}

/// Each caller's mark is its own: another caller's commit, or a second drop,
/// leaves it alone, and only its own Abort or Begin clears it.
#[test]
fn a_commit_or_a_second_drop_leaves_a_callers_mark() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let doc = create(&mut c, "doc").expect("a file");
    fill(&mut c);
    // Alice's action is dropped under Bob's read.
    assert_eq!(
        home(&mut c, ALICE, Request::Begin { action: 1 }).0,
        Reply::Done
    );
    assert_eq!(write_run(&mut c, &doc, 0, 100 << 10), Reply::Done);
    let _ = home(&mut c, BOB, Request::Stat);
    // Bob's own action, begun and committed: Alice is still marked.
    assert_eq!(
        home(&mut c, BOB, Request::Begin { action: 2 }).0,
        Reply::Done
    );
    let committed = home(&mut c, BOB, Request::Commit).0;
    assert!(
        matches!(committed, Reply::Committed { .. }),
        "{committed:?}"
    );
    assert_eq!(write(&mut c, &doc, 0, 10), Reply::Failed(Error::Dropped));
    // Carol's action is dropped the same way: both are marked.
    assert_eq!(
        home(&mut c, CAROL, Request::Begin { action: 3 }).0,
        Reply::Done
    );
    for at in (0..100 << 10).step_by(fsproto::MAX_DATA) {
        let request = Request::Write {
            off: at as u64,
            data: vec![5; fsproto::MAX_DATA],
        };
        assert_eq!(c.call(CAROL, doc.badge, doc.rights, request).0, Reply::Done);
    }
    let _ = home(&mut c, BOB, Request::Stat);
    assert_eq!(
        write_as(&mut c, CAROL, &doc, 10),
        Reply::Failed(Error::Dropped)
    );
    assert_eq!(write(&mut c, &doc, 0, 10), Reply::Failed(Error::Dropped));
    // Alice's Abort clears hers alone.
    assert_eq!(home(&mut c, ALICE, Request::Abort).0, Reply::Done);
    assert_ne!(write(&mut c, &doc, 0, 10), Reply::Failed(Error::Dropped));
    assert_eq!(
        write_as(&mut c, CAROL, &doc, 10),
        Reply::Failed(Error::Dropped)
    );
}

/// The owner's own run crossing a record's end goes to kaifs whole; on a full
/// disk that fails and drops the action. That Write is answered Dropped, so
/// the owner knows its action is gone, and it is not marked.
#[test]
fn the_owners_own_failed_write_is_answered_not_marked() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let doc = create(&mut c, "doc").expect("a file");
    fill(&mut c);
    assert_eq!(
        home(&mut c, ALICE, Request::Begin { action: 1 }).0,
        Reply::Done
    );
    // Past 128 KiB the first whole record is written: no room for it.
    let crossing = write_run(&mut c, &doc, 0, 150 << 10);
    assert_eq!(crossing, Reply::Failed(Error::Dropped));
    assert_ne!(write(&mut c, &doc, 0, 10), Reply::Failed(Error::Dropped));
}

/// The owner's own Commit fails writing its run out on a full disk: it hears
/// that in the Commit's answer, so its next standalone change, the person's
/// undo, is not refused.
#[test]
fn an_undo_after_the_owners_own_failed_commit_is_not_refused() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let doc = create(&mut c, "doc").expect("a file");
    fill(&mut c);
    assert_eq!(
        home(&mut c, ALICE, Request::Begin { action: 1 }).0,
        Reply::Done
    );
    assert_eq!(write_run(&mut c, &doc, 0, 100 << 10), Reply::Done);
    let commit = home(&mut c, ALICE, Request::Commit).0;
    assert!(matches!(commit, Reply::Failed(_)), "{commit:?}");
    let undo = Request::Restore { txn: 1, action: 2 };
    assert_ne!(home(&mut c, ALICE, undo).0, Reply::Failed(Error::Dropped));
}

/// The pid [`a_dead_callers_mark_is_forgotten`] has ended.
static ENDED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn alive(pid: u64) -> bool {
    pid != ENDED.load(core::sync::atomic::Ordering::Relaxed)
}

/// A marked caller that dies is forgotten at the next mark, so a process
/// given its pid later is not refused.
#[test]
fn a_dead_callers_mark_is_forgotten() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    c.server.set_alive(alive);
    let doc = create(&mut c, "doc").expect("a file");
    fill(&mut c);
    assert_eq!(
        home(&mut c, ALICE, Request::Begin { action: 1 }).0,
        Reply::Done
    );
    assert_eq!(write_run(&mut c, &doc, 0, 100 << 10), Reply::Done);
    let _ = home(&mut c, BOB, Request::Stat);
    assert_eq!(write(&mut c, &doc, 0, 10), Reply::Failed(Error::Dropped));
    // Alice ends; Carol is dropped the same way, which forgets Alice.
    ENDED.store(ALICE, core::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        home(&mut c, CAROL, Request::Begin { action: 2 }).0,
        Reply::Done
    );
    for at in (0..100 << 10).step_by(fsproto::MAX_DATA) {
        let request = Request::Write {
            off: at as u64,
            data: vec![5; fsproto::MAX_DATA],
        };
        assert_eq!(c.call(CAROL, doc.badge, doc.rights, request).0, Reply::Done);
    }
    let _ = home(&mut c, BOB, Request::Stat);
    ENDED.store(0, core::sync::atomic::Ordering::Relaxed);
    // A process with Alice's pid now is not refused.
    assert_ne!(write(&mut c, &doc, 0, 10), Reply::Failed(Error::Dropped));
}
