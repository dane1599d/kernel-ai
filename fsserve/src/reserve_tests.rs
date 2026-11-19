//! kaifs's reserve through `fs`: a disk filled by clients' growth can still be
//! emptied, since a trash and an emptied trash may take the reserve for the
//! commits they need, and idle's commits give the freed blocks back.

use super::tests::{ALICE, Client};
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
