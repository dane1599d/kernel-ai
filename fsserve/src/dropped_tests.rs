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
    assert_eq!(
        write(&mut c, &doc, 200, 10),
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
    );
    // (A Commit would be told Dropped and clear the mark too; see
    // `a_marked_owners_commit_is_told_dropped_and_its_undo_goes_through`.)
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
    assert_ne!(
        write(&mut c, &doc, 0, 10),
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
    );
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
    assert_ne!(
        home(&mut c, BOB, trash).0,
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
    );
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
    assert_eq!(
        write(&mut c, &doc, 0, 10),
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
    );
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
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
    );
    assert_eq!(
        write(&mut c, &doc, 0, 10),
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
    );
    // Alice's Abort clears hers alone.
    assert_eq!(home(&mut c, ALICE, Request::Abort).0, Reply::Done);
    assert_ne!(
        write(&mut c, &doc, 0, 10),
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
    );
    assert_eq!(
        write_as(&mut c, CAROL, &doc, 10),
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
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
    assert_eq!(crossing, Reply::Failed(Error::Dropped(DropCause::NoSpace)));
    assert_ne!(
        write(&mut c, &doc, 0, 10),
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
    );
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
    assert_ne!(
        home(&mut c, ALICE, undo).0,
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
    );
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
    assert_eq!(
        write(&mut c, &doc, 0, 10),
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
    );
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
    assert_ne!(
        write(&mut c, &doc, 0, 10),
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
    );
}

/// The shell's line, as it keeps its action: begun by its first change, ended
/// when a change's error ends it ([`Error::ends_action`]).
struct Line {
    open: bool,
}

impl Line {
    fn change(&mut self, c: &mut Client<'_>, doc: &Mint, off: u64, len: usize) -> Reply {
        if !self.open {
            assert_eq!(home(c, ALICE, Request::Begin { action: 9 }).0, Reply::Done);
            self.open = true;
        }
        let reply = write_run(c, doc, off, len);
        if matches!(&reply, Reply::Failed(e) if e.ends_action()) {
            self.open = false;
        }
        reply
    }
}

/// A line whose own write drops its action, and that catches the error and
/// writes again, as a script can: the next write begins a new action, never
/// landing alone outside one.
#[test]
fn a_line_that_writes_on_after_a_drop_begins_a_new_action() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let doc = create(&mut c, "doc").expect("a file");
    fill(&mut c);
    let mut line = Line { open: false };
    let dropped = line.change(&mut c, &doc, 0, 150 << 10);
    assert_eq!(dropped, Reply::Failed(Error::Dropped(DropCause::NoSpace)));
    assert!(!line.open, "the line still thinks its action is open");
    assert_eq!(line.change(&mut c, &doc, 0, 10), Reply::Done);
    assert_eq!(
        c.server.owner,
        Some(ALICE),
        "the write landed outside an action"
    );
    assert_eq!(home(&mut c, ALICE, Request::Abort).0, Reply::Done);
}

/// A marked owner's Commit is told its action is gone, and that clears the
/// mark: its undo, the next standalone change, is not refused.
#[test]
fn a_marked_owners_commit_is_told_dropped_and_its_undo_goes_through() {
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
    assert_eq!(
        home(&mut c, ALICE, Request::Commit).0,
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
    );
    let undo = Request::Restore { txn: 1, action: 2 };
    assert_ne!(
        home(&mut c, ALICE, undo).0,
        Reply::Failed(Error::Dropped(DropCause::NoSpace))
    );
}

/// The owner's own read writes out its pending run first; when that drops
/// the action, the read is answered Dropped, a kind that ends it.
#[test]
fn the_owners_read_that_drops_its_action_is_answered_dropped() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let doc = create(&mut c, "doc").expect("a file");
    fill(&mut c);
    assert_eq!(
        home(&mut c, ALICE, Request::Begin { action: 1 }).0,
        Reply::Done
    );
    assert_eq!(write_run(&mut c, &doc, 0, 100 << 10), Reply::Done);
    let read = home(&mut c, ALICE, Request::Stat).0;
    assert_eq!(read, Reply::Failed(Error::Dropped(DropCause::NoSpace)));
    assert!(matches!(&read, Reply::Failed(e) if e.ends_action()));
    assert_eq!(c.server.owner, None);
}

/// H3-11: `fs` restarts while Alice's action is open. A caller older than the
/// new `fs` is told, on its first change, that any action it had is gone, and
/// that change lands nothing: nothing she meant for the action commits on its
/// own. Once told, her changes land; a newer caller and the kernel never are.
#[test]
fn a_restart_tells_an_older_callers_first_change_its_action_is_gone() {
    let mut disk = MemDisk::new(2 * 1024);
    let doc = {
        let (mut c, _) = Client::start(&mut disk);
        let doc = create(&mut c, "doc").expect("a file");
        let begin = Request::Begin { action: 1 };
        assert_eq!(home(&mut c, ALICE, begin).0, Reply::Done);
        assert_eq!(write(&mut c, &doc, 0, 3), Reply::Done);
        doc
    };
    // The first `fs` is gone with her action open; this one started after her.
    let (mut c, _) = Client::start(&mut disk);
    c.server.set_own_pid(100);
    let before = c.server.generation();
    let dropped = Reply::Failed(Error::Dropped(DropCause::Io));
    assert_eq!(write(&mut c, &doc, 3, 3), dropped);
    assert_eq!(c.server.generation(), before, "the refused write committed");
    assert_eq!(write(&mut c, &doc, 0, 3), Reply::Done);
    // An older caller's Commit is its action's end; its Begin is a fresh start.
    assert_eq!(home(&mut c, CAROL, Request::Commit).0, dropped);
    let begin = Request::Begin { action: 2 };
    assert_eq!(home(&mut c, BOB, begin).0, Reply::Done);
    let committed = home(&mut c, BOB, Request::Commit).0;
    assert!(
        matches!(committed, Reply::Committed { .. }),
        "{committed:?}"
    );
    for caller in [0, 200] {
        let open = Request::Open {
            path: format!("new{caller}"),
            rights: DIR_RIGHTS.bits(),
            how: How::CreateFile,
        };
        let opened = home(&mut c, caller, open).0;
        assert!(matches!(opened, Reply::Opened(..)), "{caller}: {opened:?}");
    }
}
