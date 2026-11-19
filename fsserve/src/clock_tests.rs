//! kaifs's clock from the wall clock (`CLOCK_NOW`): undo snapshots expire 30
//! days (`memory`'s one hour) after they were made, counting the time the
//! machine was off, and a wall clock behind the disk's never moves it back.

use super::tests::{ALICE, BLOCKS, Client};
use super::*;
use alloc::vec::Vec;
use kaifs::vol::{HOME, MEMORY, SnapKind, UNDO_LIFE};
use kaifs::{MemDisk, ROOT};

const HOUR: u64 = 3600 * 1_000_000_000;
const DAY: u64 = 24 * HOUR;
/// A wall time at boot, ns since 1970 (late 2026).
const BOOT: u64 = 1_795_000_000 * 1_000_000_000;

/// A server on `disk` whose kaifs clock runs from `wall`, if given, with its
/// clock moved by one call.
fn started(disk: &mut MemDisk, wall: Option<u64>) -> Client<'_> {
    let (mut c, _) = Client::start(disk);
    if let Some(wall) = wall {
        c.server.set_wall_clock(wall);
    }
    let stat = c.call(ALICE, HOME_USER, DIR_RIGHTS, Request::Stat).0;
    assert!(matches!(stat, Reply::Stat(_)), "{stat:?}");
    c
}

/// The expiry of each undo snapshot of `volume`.
fn expiries(c: &mut Client<'_>, volume: u64) -> Vec<u64> {
    let fs = c.server.fs();
    let active = fs.volume();
    fs.set_volume(volume).unwrap();
    let undo = fs
        .snapshots()
        .values()
        .filter(|s| s.kind == SnapKind::Undo)
        .map(|s| s.expiry)
        .collect();
    fs.set_volume(active).unwrap();
    undo
}

/// One action adding file `name` to `memory` and `home`, committed at the
/// clock now; the time it was made.
fn one_action(c: &mut Client<'_>, name: &str) -> u64 {
    let fs = c.server.fs();
    let active = fs.volume();
    let made = fs.clock;
    fs.begin(1).unwrap();
    for volume in [MEMORY, HOME] {
        fs.set_volume(volume).unwrap();
        let f = fs.create(ROOT, name).unwrap();
        fs.write_file(f, b"likes tea").unwrap();
    }
    fs.commit().unwrap();
    fs.set_volume(active).unwrap();
    made
}

#[test]
fn undo_snapshots_expire_on_the_wall_clock_however_long_the_machine_was_off() {
    let mut disk = MemDisk::new(BLOCKS);
    let made = {
        let mut c = started(&mut disk, Some(BOOT));
        let made = one_action(&mut c, "a");
        assert!(made >= BOOT, "the clock runs from the wall time");
        assert_eq!(expiries(&mut c, MEMORY), [made + HOUR]);
        assert_eq!(expiries(&mut c, HOME), [made + UNDO_LIFE]);
        made
    };
    // Without a date, only the time `fs` runs counts: a minute's run keeps both.
    {
        let mut c = started(&mut disk, None);
        c.server.idle(60_000_000_000);
        assert_eq!(expiries(&mut c, MEMORY).len(), 1);
    }
    // Off for two hours: `memory`'s hour has passed, `home`'s 30 days have not.
    {
        let mut c = started(&mut disk, Some(made + 2 * HOUR));
        c.server.idle(1_000_000);
        assert!(expiries(&mut c, MEMORY).is_empty());
        assert_eq!(expiries(&mut c, HOME), [made + UNDO_LIFE]);
    }
    // Off for 31 days: `home`'s has passed too.
    let mut c = started(&mut disk, Some(made + 31 * DAY));
    c.server.idle(1_000_000);
    assert!(expiries(&mut c, HOME).is_empty());
}

#[test]
fn a_wall_clock_behind_the_disks_never_moves_kaifs_back() {
    let mut disk = MemDisk::new(BLOCKS);
    let ahead = {
        let mut c = started(&mut disk, Some(BOOT + 10 * DAY));
        one_action(&mut c, "a");
        c.server.fs().clock
    };
    // The firmware's date is ten days behind the disk: the clock goes on from
    // the disk's, and so does an action's expiry.
    let mut c = started(&mut disk, Some(BOOT));
    assert!(c.server.fs().clock >= ahead);
    let made = one_action(&mut c, "b");
    assert!(made >= ahead);
    assert!(expiries(&mut c, HOME).contains(&(made + UNDO_LIFE)));
}
