//! A handle minted for an object made in an action that was then dropped,
//! by Abort or by `fs` stopping mid-action, never comes to name another
//! caller's object: object ids are never handed out twice.

use super::tests::{ALICE, BLOCKS, BOB, Client};
use super::*;
use kaifs::MemDisk;

fn home(c: &mut Client<'_>, caller: u64, request: Request) -> (Reply, Option<Mint>) {
    c.call(caller, HOME_USER, DIR_RIGHTS, request)
}

fn create(c: &mut Client<'_>, caller: u64, path: &str, how: How) -> Mint {
    let open = Request::Open {
        path: path.into(),
        rights: DIR_RIGHTS.bits(),
        how,
    };
    match home(c, caller, open) {
        (Reply::Opened(..), Some(mint)) => mint,
        other => panic!("{other:?}"),
    }
}

/// What a stale handle gets: the object it named is gone, and nothing else
/// answers to its id.
fn stale(c: &mut Client<'_>, mint: &Mint) -> Reply {
    c.call(ALICE, mint.badge, mint.rights, Request::Stat).0
}

#[test]
fn handles_from_an_aborted_action_never_open_a_later_object() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    // Alice makes a file and a folder in an action, keeps the handles, and
    // drops the action.
    assert_eq!(
        home(&mut c, ALICE, Request::Begin { action: 1 }).0,
        Reply::Done
    );
    let file = create(&mut c, ALICE, "t", How::CreateFile);
    let dir = create(&mut c, ALICE, "d", How::Mkdir);
    assert_eq!(home(&mut c, ALICE, Request::Abort).0, Reply::Done);
    // Bob then makes his own, outside any action.
    let victim = create(&mut c, BOB, "victim", How::CreateFile);
    let folder = create(&mut c, BOB, "folder", How::Mkdir);
    assert_ne!(victim.badge, file.badge);
    assert_ne!(folder.badge, dir.badge);
    for mint in [&file, &dir] {
        assert_eq!(stale(&mut c, mint), Reply::Failed(Error::NotFound));
    }
}

#[test]
fn handles_from_an_action_fs_never_finished_never_open_a_later_object() {
    let mut disk = MemDisk::new(BLOCKS);
    let (file, dir) = {
        let (mut c, _) = Client::start(&mut disk);
        assert_eq!(
            home(&mut c, ALICE, Request::Begin { action: 1 }).0,
            Reply::Done
        );
        let file = create(&mut c, ALICE, "t", How::CreateFile);
        let dir = create(&mut c, ALICE, "d", How::Mkdir);
        // `fs` stops here, the action open: nothing of it was committed.
        (file, dir)
    };
    let (mut c, _) = Client::start(&mut disk);
    let victim = create(&mut c, BOB, "victim", How::CreateFile);
    let folder = create(&mut c, BOB, "folder", How::Mkdir);
    assert_ne!(victim.badge, file.badge);
    assert_ne!(folder.badge, dir.badge);
    for mint in [&file, &dir] {
        assert_eq!(stale(&mut c, mint), Reply::Failed(Error::NotFound));
    }
}
