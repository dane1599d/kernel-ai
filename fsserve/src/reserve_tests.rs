//! kaifs's reserve through `fs`: a disk filled by clients' growth can still be
//! emptied, since a trash and an emptied trash may take the reserve for the
//! commits they need, and idle's commits give the freed blocks back.

use super::tests::{ALICE, BOB, Client};
use super::*;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use kaifs::MemDisk;

fn home(c: &mut Client<'_>, request: Request) -> Reply {
    c.call(ALICE, HOME_USER, DIR_RIGHTS, request).0
}

/// Writes 4 KiB files under `/home/user` until growth is refused; their names.
fn fill(c: &mut Client<'_>) -> Vec<String> {
    let mut made = Vec::new();
    for n in 0..10_000 {
        let open = Request::Open {
            path: format!("f{n}"),
            rights: DIR_RIGHTS.bits(),
            how: How::CreateFile,
        };
        let (reply, mint) = c.call(ALICE, HOME_USER, DIR_RIGHTS, open);
        let Some(mint) = mint else {
            assert_eq!(reply, Reply::Failed(Error::NoSpace));
            return made;
        };
        let write = Request::Write {
            off: 0,
            data: vec![7; 4096],
        };
        match c.call(ALICE, mint.badge, mint.rights, write).0 {
            Reply::Done => made.push(format!("f{n}")),
            other => {
                assert_eq!(other, Reply::Failed(Error::NoSpace));
                return made;
            }
        }
    }
    panic!("the disk never filled");
}

#[test]
fn a_disk_filled_by_growth_can_still_be_emptied_through_fs() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let made = fill(&mut c);
    assert!(made.len() > 100, "{} files", made.len());
    let (full, _) = c.server.fs().space();
    // A trash frees nothing until the trash is emptied, so trashing may use
    // only half the reserve: past that it is refused, and the emptying still
    // has the rest.
    let mut trashed = 0;
    for name in &made {
        let trash = Request::Trash { name: name.clone() };
        match home(&mut c, trash) {
            Reply::Done => trashed += 1,
            Reply::Failed(Error::NoSpace) => break,
            other => panic!("trashing {name}: {other:?}"),
        }
    }
    assert!(trashed > 10, "only {trashed} trashed on a full disk");
    let emptied = home(&mut c, Request::EmptyTrash);
    assert!(matches!(emptied, Reply::Emptied { .. }), "{emptied:?}");
    for round in 0..20u64 {
        c.server.idle(1_000_000 + round);
    }
    let (freed, _) = c.server.fs().space();
    assert!(freed > full, "{full} free full, {freed} after");
    // Growth works again.
    let open = Request::Open {
        path: "again".into(),
        rights: DIR_RIGHTS.bits(),
        how: How::CreateFile,
    };
    assert!(matches!(home(&mut c, open), Reply::Opened(..)));
}

/// The tiers each request gets: all of the reserve only for work that frees
/// space at once, half for a trash or a truncate, none for growth or a Sync
/// outside an action, and an action's commit (by Commit or its owner's Sync)
/// the least of its changes'.
#[test]
fn each_request_gets_its_tier_and_a_commit_the_least_of_its_actions() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let tier = |c: &mut Client<'_>, caller: u64, request: Request| {
        c.server.spends_reserve(caller, &request, changes(&request))
    };
    let trash = || Request::Trash { name: "x".into() };
    let truncate = || Request::Truncate { size: 0 };
    let write = || Request::Write {
        off: 0,
        data: vec![1],
    };
    assert_eq!(tier(&mut c, ALICE, Request::EmptyTrash), Spend::All);
    assert_eq!(
        tier(&mut c, ALICE, Request::UpdateDrop { id: 1 }),
        Spend::All
    );
    assert_eq!(tier(&mut c, ALICE, trash()), Spend::Half);
    assert_eq!(tier(&mut c, ALICE, truncate()), Spend::Half);
    assert_eq!(tier(&mut c, ALICE, Request::Sync), Spend::Keep);
    assert_eq!(tier(&mut c, ALICE, write()), Spend::Keep);
    // An action of trashes and truncates commits at half; Bob's Sync in it,
    // which is not his, keeps to the reserve.
    assert_eq!(home(&mut c, Request::Begin { action: 1 }), Reply::Done);
    tier(&mut c, ALICE, trash());
    tier(&mut c, ALICE, truncate());
    assert_eq!(tier(&mut c, ALICE, Request::Sync), Spend::Half);
    assert_eq!(tier(&mut c, BOB, Request::Sync), Spend::Keep);
    assert_eq!(tier(&mut c, ALICE, Request::Commit), Spend::Half);
    assert_eq!(home(&mut c, Request::Abort), Reply::Done);
    // One write makes the whole action growth.
    assert_eq!(home(&mut c, Request::Begin { action: 2 }), Reply::Done);
    tier(&mut c, ALICE, Request::EmptyTrash);
    tier(&mut c, ALICE, write());
    assert_eq!(tier(&mut c, ALICE, Request::Commit), Spend::Keep);
    assert_eq!(home(&mut c, Request::Abort), Reply::Done);
    // Only emptying: all of it.
    assert_eq!(home(&mut c, Request::Begin { action: 3 }), Reply::Done);
    tier(&mut c, ALICE, Request::EmptyTrash);
    assert_eq!(tier(&mut c, ALICE, Request::Commit), Spend::All);
    assert_eq!(home(&mut c, Request::Abort), Reply::Done);
}

/// A hostile app loops a truncate to the same size and a Sync on a full
/// disk: neither drains more than half the reserve, and emptying the trash
/// still works afterwards.
#[test]
fn a_truncate_and_sync_loop_never_drains_the_reserve() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let open = Request::Open {
        path: "victim".into(),
        rights: DIR_RIGHTS.bits(),
        how: How::CreateFile,
    };
    let (_, victim) = c.call(ALICE, HOME_USER, DIR_RIGHTS, open);
    let victim = victim.expect("a file");
    let write = Request::Write {
        off: 0,
        data: vec![7; 4096],
    };
    assert_eq!(
        c.call(ALICE, victim.badge, victim.rights, write).0,
        Reply::Done
    );
    let made = fill(&mut c);
    for _ in 0..300 {
        let truncate = Request::Truncate { size: 4096 };
        let _ = c.call(ALICE, victim.badge, victim.rights, truncate);
        let _ = home(&mut c, Request::Sync);
    }
    let reserve = c.server.fs().io.alloc.reserve();
    let (free, _) = c.server.fs().space();
    assert!(free >= reserve / 2, "{free} free, the reserve {reserve}");
    let trash = Request::Trash {
        name: made[0].clone(),
    };
    assert_eq!(home(&mut c, trash), Reply::Done);
    let emptied = home(&mut c, Request::EmptyTrash);
    assert!(matches!(emptied, Reply::Emptied { .. }), "{emptied:?}");
}

/// A shared-memory push of a model blob is growth, as a message's push is: it
/// gets only what is above the reserve.
#[test]
fn a_shared_memory_push_keeps_the_reserve() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let push = Request::SealedPushShm { len: 4096 };
    let tier = c.server.spends_reserve(ALICE, &push, changes(&push));
    assert_eq!(tier, Spend::Keep);
}
