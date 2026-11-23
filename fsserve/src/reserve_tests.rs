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

/// A pid the tests' kernel knows as an app with no special caps.
const CAROL: u64 = 9;

/// The tests' PART_PEER_CAPS: Alice is the person (the shell's POLICY), Bob
/// and Carol are apps, any other pid is not running.
fn caps(pid: u64) -> Option<u64> {
    match pid {
        ALICE => Some(kabi::cap::SERVE | kabi::cap::POLICY),
        BOB | CAROL => Some(kabi::cap::CALL),
        _ => None,
    }
}

/// Bob's process name, for his one folder of `/system/apps`.
fn bob_named(pid: u64, out: &mut [u8; kabi::MAX_NAME]) -> Option<usize> {
    let name = b"files-window";
    (pid == BOB).then(|| {
        out[..name.len()].copy_from_slice(name);
        name.len()
    })
}

/// The person (the caller holding `cap::POLICY`) gets a quarter for a trash or
/// a truncate where an app gets half, an action of only those commits at it,
/// and the kernel, an app and a pid not running are not the person.
#[test]
fn the_person_s_trash_and_truncate_go_to_their_quarter() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let tier = |c: &mut Client<'_>, caller: u64, request: Request| {
        c.server.spends_reserve(caller, &request, changes(&request))
    };
    let trash = || Request::Trash { name: "x".into() };
    let truncate = || Request::Truncate { size: 0 };
    // Until `fs` is told how to ask, nobody is the person.
    assert_eq!(tier(&mut c, ALICE, trash()), Spend::Half);
    c.server.set_caps_of(caps);
    assert_eq!(tier(&mut c, ALICE, trash()), Spend::Person);
    assert_eq!(tier(&mut c, ALICE, truncate()), Spend::Person);
    for caller in [0, BOB, CAROL, 1234] {
        assert_eq!(tier(&mut c, caller, trash()), Spend::Half, "{caller}");
        assert_eq!(tier(&mut c, caller, truncate()), Spend::Half, "{caller}");
    }
    // Growth and the emptying are the person's as anyone's.
    let write = Request::Write {
        off: 0,
        data: vec![1],
    };
    assert_eq!(tier(&mut c, ALICE, write.clone()), Spend::Keep);
    assert_eq!(tier(&mut c, ALICE, Request::EmptyTrash), Spend::All);
    // The person's action of trashes commits at their quarter; one write in it
    // makes it growth.
    assert_eq!(home(&mut c, Request::Begin { action: 1 }), Reply::Done);
    tier(&mut c, ALICE, trash());
    tier(&mut c, ALICE, truncate());
    assert_eq!(tier(&mut c, ALICE, Request::Commit), Spend::Person);
    tier(&mut c, ALICE, write);
    assert_eq!(tier(&mut c, ALICE, Request::Commit), Spend::Keep);
    assert_eq!(home(&mut c, Request::Abort), Reply::Done);
    // The answers are kept: the kernel is asked once a caller.
    assert_eq!(
        c.server.people.iter().filter(|(p, _)| *p == ALICE).count(),
        1
    );
    assert!(!c.server.people.iter().any(|(p, _)| *p == 0));
    // Kept for the latest few only, and asked again after.
    for pid in 100..100 + PEOPLE_KEPT as u64 {
        assert_eq!(tier(&mut c, pid, trash()), Spend::Half);
    }
    assert_eq!(c.server.people.len(), PEOPLE_KEPT);
    assert!(!c.server.people.iter().any(|(p, _)| *p == ALICE));
    assert_eq!(tier(&mut c, ALICE, trash()), Spend::Person);
}

/// #1204's review HIGH: an app trashes its own files on a full disk until it
/// is refused, which spends the apps' half of the reserve. Another app's trash
/// is then refused too, but the person's goes into their own quarter: they can
/// still delete, the last quarter stays for the emptying, and the emptying
/// works.
#[test]
fn an_app_trashing_until_refused_leaves_the_person_room_to_delete() {
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    c.server.set_name_of(bob_named);
    c.server.set_caps_of(caps);
    // Bob's folder of `/system/apps`, with many small files in it.
    let open = Request::Open {
        path: "files-window".into(),
        rights: DIR_RIGHTS.bits(),
        how: How::Mkdir,
    };
    let (reply, own) = c.call(BOB, kabi::fs::SYSTEM_APPS, DIR_RIGHTS, open);
    let own = own.unwrap_or_else(|| panic!("{reply:?}"));
    let mut his = Vec::new();
    for n in 0..600 {
        let open = Request::Open {
            path: format!("b{n}"),
            rights: DIR_RIGHTS.bits(),
            how: How::CreateFile,
        };
        let (reply, file) = c.call(BOB, own.badge, own.rights, open);
        assert!(file.is_some(), "b{n}: {reply:?}");
        his.push(format!("b{n}"));
    }
    let made = fill(&mut c);
    let reserve = c.server.fs().io.alloc.reserve();
    // Bob trashes until refused.
    let mut trashed = 0;
    for name in &his {
        let trash = Request::Trash { name: name.clone() };
        match c.call(BOB, own.badge, own.rights, trash).0 {
            Reply::Done => trashed += 1,
            Reply::Failed(Error::NoSpace) => break,
            other => panic!("Bob trashing {name}: {other:?}"),
        }
    }
    assert!(
        trashed > 10 && trashed < his.len(),
        "Bob trashed {trashed} of {}",
        his.len()
    );
    let (free, _) = c.server.fs().space();
    assert!(free >= reserve / 2, "{free} free, the reserve {reserve}");
    // Another app is refused in /home/user as well: the half is spent.
    let trash = |name: &str| Request::Trash { name: name.into() };
    assert_eq!(
        c.call(CAROL, HOME_USER, DIR_RIGHTS, trash(&made[0])).0,
        Reply::Failed(Error::NoSpace)
    );
    // The person still deletes, and until refused stops at their quarter.
    assert_eq!(home(&mut c, trash(&made[0])), Reply::Done);
    let mut theirs = 1;
    for name in &made[1..] {
        match home(&mut c, trash(name)) {
            Reply::Done => theirs += 1,
            Reply::Failed(Error::NoSpace) => break,
            other => panic!("the person trashing {name}: {other:?}"),
        }
    }
    assert!(theirs < made.len(), "the person's quarter never ran out");
    let (free, _) = c.server.fs().space();
    assert!(free >= reserve / 4, "{free} free, the reserve {reserve}");
    // The emptying has the last quarter.
    let emptied = home(&mut c, Request::EmptyTrash);
    assert!(matches!(emptied, Reply::Emptied { .. }), "{emptied:?}");
}
