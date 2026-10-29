//! The server over kaifs's in-memory disk: what a client sees through `kai-fs 1`,
//! and what a restarted `fs` serves from the same disk.

use super::*;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use fsproto::{decode_reply, encode_request};
use kaifs::MemDisk;

const BLOCKS: u64 = 16 * 1024;
const ALICE: u64 = 7;
const BOB: u64 = 8;

struct Client<'d> {
    server: Server<&'d mut MemDisk>,
    now: u64,
}

impl<'d> Client<'d> {
    fn start(disk: &'d mut MemDisk) -> (Self, Started) {
        let (server, started) = Server::start(disk, [7; 16]).unwrap();
        (Client { server, now: 0 }, started)
    }

    fn call(
        &mut self,
        caller: u64,
        badge: u64,
        rights: Rights,
        request: Request,
    ) -> (Reply, Option<Mint>) {
        self.now += 1_000;
        let bytes = encode_request(&request);
        let answer = self.server.call(&Call {
            caller,
            badge,
            rights,
            request: &bytes,
            now: self.now,
        });
        (decode_reply(&answer.reply).unwrap(), answer.mint)
    }

    fn home(&mut self, request: Request) -> (Reply, Option<Mint>) {
        self.call(ALICE, HOME_USER, DIR_RIGHTS, request)
    }

    /// Opens `path` under /home/user; the new handle's badge and rights.
    fn open(&mut self, path: &str, how: How) -> Result<Mint, Error> {
        let rights = DIR_RIGHTS.bits();
        match self.home(Request::Open {
            path: path.into(),
            rights,
            how,
        }) {
            (Reply::Opened(..), Some(mint)) => Ok(mint),
            (Reply::Failed(e), None) => Err(e),
            other => panic!("{other:?}"),
        }
    }

    fn write(&mut self, file: &Mint, off: u64, data: &[u8]) -> Reply {
        self.call(
            ALICE,
            file.badge,
            file.rights,
            Request::Write {
                off,
                data: data.to_vec(),
            },
        )
        .0
    }

    fn read(&mut self, file: &Mint) -> Result<Vec<u8>, Error> {
        match self
            .call(
                ALICE,
                file.badge,
                file.rights,
                Request::Read { off: 0, len: 4096 },
            )
            .0
        {
            Reply::Data(data) => Ok(data),
            Reply::Failed(e) => Err(e),
            other => panic!("{other:?}"),
        }
    }
}

fn failed(reply: (Reply, Option<Mint>)) -> Error {
    match reply {
        (Reply::Failed(e), None) => e,
        other => panic!("not a failure: {other:?}"),
    }
}

#[test]
fn a_blank_disk_is_formatted_and_a_file_survives_a_restart_with_its_badge() {
    let mut disk = MemDisk::new(BLOCKS);
    let badge = {
        let (mut c, started) = Client::start(&mut disk);
        assert_eq!(started, Started::Formatted);
        let file = c.open("plan.txt", How::CreateFile).unwrap();
        assert!(file.file);
        assert_eq!(file.rights, FILE_RIGHTS);
        assert_eq!(c.write(&file, 0, b"buy milk"), Reply::Done);
        file.badge
    };
    // A restarted fs: the same disk, nothing kept in memory. The write was
    // answered after its commit, so it is there; the old badge still works.
    let (mut c, started) = Client::start(&mut disk);
    assert!(matches!(started, Started::Mounted { .. }));
    let file = Mint {
        file: true,
        rights: FILE_RIGHTS,
        badge,
    };
    assert_eq!(c.read(&file), Ok(b"buy milk".to_vec()));
    match c
        .home(Request::List {
            after: None,
            max: 8,
        })
        .0
    {
        Reply::Listed { names, more: None } => {
            assert_eq!(names.len(), 1);
            assert_eq!(names[0].name, "plan.txt");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_disk_with_anything_in_a_superblock_slot_is_never_formatted() {
    let mut disk = MemDisk::new(BLOCKS);
    disk.write(BLOCKS - 1, &[1; kaifs::BLOCK]).unwrap();
    assert!(Server::start(&mut disk, [0; 16]).is_err());
}

#[test]
fn a_path_never_leaves_its_dir() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    c.open("notes", How::Mkdir).unwrap();
    c.open("notes/a.txt", How::CreateFile).unwrap();
    for (path, why) in [
        ("..", Error::Invalid),
        ("../user", Error::Invalid),
        ("notes/../../x", Error::Invalid),
        ("/home/user", Error::Invalid),
        ("notes//a.txt", Error::Invalid),
        ("missing/a.txt", Error::NotFound),
        ("notes/a.txt/x", Error::NotDir),
    ] {
        assert_eq!(c.open(path, How::Existing), Err(why), "{path}");
    }
    let a = c.open("notes/./../notes/a.txt", How::Existing).unwrap();
    assert!(a.file);
    let here = c.open("notes/..", How::Existing).unwrap();
    assert!(!here.file);
    // The handle's folder is /home/user itself, by its real object id.
    assert_eq!(Badge::parse(here.badge).object, c.server.home_user);
}

#[test]
fn create_never_overwrites() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let file = c.open("a.txt", How::CreateFile).unwrap();
    c.write(&file, 0, b"one");
    assert_eq!(c.open("a.txt", How::CreateFile), Err(Error::Exists));
    assert_eq!(c.open("a.txt", How::Mkdir), Err(Error::Exists));
    assert_eq!(c.read(&file), Ok(b"one".to_vec()));
}

#[test]
fn rights_only_narrow_and_each_request_needs_its_own() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let file = c.open("a.txt", How::CreateFile).unwrap();
    let read_only = Rights::READ | Rights::INSPECT;
    let reader = Rights::READ | Rights::ENUMERATE;
    // Asking for more than the Dir holds.
    let wider = c.call(
        ALICE,
        HOME_USER,
        reader,
        Request::Open {
            path: "a.txt".into(),
            rights: (Rights::READ | Rights::WRITE).bits(),
            how: How::Existing,
        },
    );
    assert_eq!(failed(wider), Error::Permission);
    // A Dir that cannot create.
    let make = c.call(
        ALICE,
        HOME_USER,
        reader,
        Request::Open {
            path: "b.txt".into(),
            rights: 0,
            how: How::CreateFile,
        },
    );
    assert_eq!(failed(make), Error::Permission);
    let w = c.call(
        ALICE,
        file.badge,
        read_only,
        Request::Write {
            off: 0,
            data: vec![1],
        },
    );
    assert_eq!(failed(w), Error::Permission);
    let l = c.call(
        ALICE,
        HOME_USER,
        Rights::READ,
        Request::List {
            after: None,
            max: 4,
        },
    );
    assert_eq!(failed(l), Error::Permission);
    let t = c.call(
        ALICE,
        HOME_USER,
        reader,
        Request::Trash {
            name: "a.txt".into(),
        },
    );
    assert_eq!(failed(t), Error::Permission);
    // A file opened through a narrow Dir gets no more than it asked for.
    let (_, mint) = c.call(
        ALICE,
        HOME_USER,
        DIR_RIGHTS,
        Request::Open {
            path: "a.txt".into(),
            rights: read_only.bits(),
            how: How::Existing,
        },
    );
    assert_eq!(mint.unwrap().rights, read_only | BASE);
}

#[test]
fn badges_of_another_volume_kind_or_a_gone_object_find_nothing() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let file = c.open("a.txt", How::CreateFile).unwrap();
    let real = Badge::parse(file.badge);
    let other = |change: fn(&mut Badge)| {
        let mut badge = real;
        change(&mut badge);
        badge.raw()
    };
    for badge in [
        other(|b| b.volume = 9),
        other(|b| b.file = false),
        other(|b| b.object += 1000),
        other(|b| b.object = 0),
        // Another disk's, or this one's before it was formatted again.
        other(|b| b.epoch ^= 1),
        other(|b| b.epoch = 0),
    ] {
        assert_eq!(
            failed(c.call(ALICE, badge, FILE_RIGHTS, Request::Stat)),
            Error::NotFound
        );
    }
    // Junk is a bad request, not a crash.
    let answer = c.server.call(&Call {
        caller: ALICE,
        badge: HOME_USER,
        rights: DIR_RIGHTS,
        request: b"\xff\x00junk",
        now: 1,
    });
    assert_eq!(
        decode_reply(&answer.reply),
        Ok(Reply::Failed(Error::BadRequest))
    );
}

#[test]
fn an_action_is_one_callers_until_commit_or_abort() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let file = c.open("a.txt", How::CreateFile).unwrap();
    c.write(&file, 0, b"before");
    assert_eq!(c.home(Request::Begin { action: 1 }).0, Reply::Done);
    c.write(&file, 0, b"during");
    // Bob may read, not change, while Alice's action is open.
    let bob = c.call(
        BOB,
        file.badge,
        FILE_RIGHTS,
        Request::Write {
            off: 0,
            data: vec![b'x'],
        },
    );
    assert_eq!(failed(bob), Error::Busy);
    assert_eq!(
        failed(c.call(BOB, HOME_USER, DIR_RIGHTS, Request::Commit)),
        Error::Busy
    );
    assert_eq!(c.read(&file), Ok(b"during".to_vec()));
    assert_eq!(c.home(Request::Abort).0, Reply::Done);
    assert_eq!(c.read(&file), Ok(b"before".to_vec()));
    // Committed this time, then undone by a restore of action 2.
    c.home(Request::Begin { action: 2 });
    c.write(&file, 0, b"after!");
    let Reply::Committed { generation } = c.home(Request::Commit).0 else {
        panic!("not committed");
    };
    assert_eq!(failed(c.home(Request::Commit)), Error::Invalid);
    let restored = c
        .home(Request::Restore {
            txn: generation,
            action: 3,
        })
        .0;
    assert!(matches!(restored, Reply::Restored { .. }), "{restored:?}");
    assert_eq!(c.read(&file), Ok(b"before".to_vec()));
}

#[test]
fn a_trashed_file_comes_back_with_undo() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let file = c.open("todo.txt", How::CreateFile).unwrap();
    c.write(&file, 0, b"milk");
    c.home(Request::Begin { action: 5 });
    assert_eq!(
        c.home(Request::Trash {
            name: "todo.txt".into()
        })
        .0,
        Reply::Done
    );
    let Reply::Committed { generation } = c.home(Request::Commit).0 else {
        panic!("not committed");
    };
    assert_eq!(c.open("todo.txt", How::Existing), Err(Error::NotFound));
    let restored = c
        .home(Request::Restore {
            txn: generation,
            action: 6,
        })
        .0;
    assert!(matches!(restored, Reply::Restored { .. }), "{restored:?}");
    let back = c.open("todo.txt", How::Existing).unwrap();
    assert_eq!(c.read(&back), Ok(b"milk".to_vec()));
}

#[test]
fn rename_in_place_refuses_a_taken_name() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    c.open("a", How::CreateFile).unwrap();
    c.open("b", How::CreateFile).unwrap();
    let taken = c.home(Request::Rename {
        from: "a".into(),
        to: "b".into(),
    });
    assert_eq!(failed(taken), Error::Exists);
    let moved = c.home(Request::Rename {
        from: "a".into(),
        to: "c".into(),
    });
    assert_eq!(moved.0, Reply::Done);
    assert!(c.open("c", How::Existing).is_ok());
}

#[test]
fn a_long_listing_goes_on_from_its_cursor() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    for i in 0..40 {
        c.open(&format!("f{i:02}"), How::CreateFile).unwrap();
    }
    let mut seen: Vec<String> = Vec::new();
    let mut after = None;
    loop {
        let Reply::Listed { names, more } = c.home(Request::List { after, max: 100 }).0 else {
            panic!("no listing");
        };
        assert!(names.len() <= MAX_LIST as usize);
        seen.extend(names.into_iter().map(|n| n.name));
        match more {
            Some(cursor) => after = Some(cursor),
            None => break,
        }
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 40);
}

#[test]
fn changes_and_history_answer_from_the_disk() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let since = c.server.generation();
    let file = c.open("a.txt", How::CreateFile).unwrap();
    c.home(Request::Begin { action: 9 });
    c.write(&file, 0, b"v2");
    c.home(Request::Commit);
    let Reply::Changes { rows, more: false } = c.home(Request::Changes { since }).0 else {
        panic!("no changes");
    };
    let object = Badge::parse(file.badge).object;
    assert!(rows.iter().any(|s| s.object == object));
    let Reply::History(versions) = c.call(ALICE, file.badge, FILE_RIGHTS, Request::History).0
    else {
        panic!("no history");
    };
    assert!(!versions.is_empty());
    let Reply::Space { free, total } = c.home(Request::Df).0 else {
        panic!("no space");
    };
    assert!(free < total && total > 0);
}

#[test]
fn a_large_file_in_small_messages_is_written_once_per_record() {
    // 4 MiB in 7 KiB pieces on an 8 MiB disk: rewriting a 128 KiB record per
    // piece would need about 70 MiB inside the action.
    let mut disk = MemDisk::new(2048);
    let (mut c, _) = Client::start(&mut disk);
    let file = c.open("big.bin", How::CreateFile).unwrap();
    let bytes: Vec<u8> = (0..4 << 20).map(|i: u32| (i * 7 % 253) as u8).collect();
    c.home(Request::Begin { action: 1 });
    for (at, piece) in bytes.chunks(MAX_DATA).enumerate() {
        let off = (at * MAX_DATA) as u64;
        assert_eq!(c.write(&file, off, piece), Reply::Done, "piece {at}");
        // A read in the middle of the run sees it.
        if at == 100 {
            let Reply::Data(back) = c
                .call(
                    ALICE,
                    file.badge,
                    FILE_RIGHTS,
                    Request::Read { off, len: 64 },
                )
                .0
            else {
                panic!("no data");
            };
            assert_eq!(back, piece[..64]);
        }
    }
    let Reply::Committed { .. } = c.home(Request::Commit).0 else {
        panic!("not committed");
    };
    let mut back = Vec::new();
    while back.len() < bytes.len() {
        let len = MAX_DATA as u32;
        let off = back.len() as u64;
        let Reply::Data(data) = c
            .call(ALICE, file.badge, FILE_RIGHTS, Request::Read { off, len })
            .0
        else {
            panic!("no data at {off}");
        };
        assert_eq!(data.len(), MAX_DATA.min(bytes.len() - back.len()));
        back.extend_from_slice(&data);
    }
    assert!(back == bytes);
}

#[test]
fn an_aborted_action_drops_its_pending_writes() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let file = c.open("a.txt", How::CreateFile).unwrap();
    c.write(&file, 0, b"kept");
    c.home(Request::Begin { action: 1 });
    c.write(&file, 0, b"gone");
    c.home(Request::Abort);
    assert_eq!(c.read(&file), Ok(b"kept".to_vec()));
}

#[test]
fn the_pkg_dir_reaches_only_beneath_system_pkg() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let pkg = |c: &mut Client, path: &str, how: How| {
        let request = Request::Open {
            path: path.into(),
            rights: (Rights::READ | Rights::WRITE).bits(),
            how,
        };
        c.call(ALICE, kabi::fs::SYSTEM_PKG, DIR_RIGHTS, request)
    };
    let (reply, mint) = pkg(&mut c, "current", How::CreateFile);
    assert!(matches!(reply, Reply::Opened(..)), "{reply:?}");
    assert_eq!(Badge::parse(mint.unwrap().badge).volume, SYSTEM);
    let (_, here) = pkg(&mut c, ".", How::Existing);
    assert_eq!(
        Badge::parse(here.unwrap().badge).object,
        c.server.system_pkg
    );
    assert_ne!(c.server.system_pkg, c.server.system_state);
    for path in ["..", "../state", "../settings", "./../pkg"] {
        assert_eq!(
            failed(pkg(&mut c, path, How::Existing)),
            Error::Invalid,
            "{path}"
        );
    }
    // The state folder does not see it.
    let state = Request::Open {
        path: "current".into(),
        rights: Rights::READ.bits(),
        how: How::Existing,
    };
    assert_eq!(
        failed(c.call(ALICE, kabi::fs::SYSTEM_STATE, DIR_RIGHTS, state)),
        Error::NotFound
    );
    // A restart finds the same folder.
    let object = c.server.system_pkg;
    drop(c);
    let (c, _) = Client::start(&mut disk);
    assert_eq!(c.server.system_pkg, object);
}

#[test]
fn the_state_dir_reaches_only_beneath_system_state() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let state = |c: &mut Client, path: &str, how: How| {
        let request = Request::Open {
            path: path.into(),
            rights: (Rights::READ | Rights::WRITE).bits(),
            how,
        };
        c.call(ALICE, kabi::fs::SYSTEM_STATE, DIR_RIGHTS, request)
    };
    let (reply, mint) = state(&mut c, "log", How::CreateFile);
    assert!(matches!(reply, Reply::Opened(..)), "{reply:?}");
    let log = Badge::parse(mint.unwrap().badge);
    assert_eq!((log.volume, log.file), (SYSTEM, true));
    // Its own folder, by its real id, and nothing beside or above it.
    let (_, here) = state(&mut c, ".", How::Existing);
    assert_eq!(
        Badge::parse(here.unwrap().badge).object,
        c.server.system_state
    );
    for path in ["..", "../settings", "../pkg", "./../state"] {
        assert_eq!(
            failed(state(&mut c, path, How::Existing)),
            Error::Invalid,
            "{path}"
        );
    }
    // The home folder does not see it, and it does not see the home folder.
    assert_eq!(c.open("log", How::Existing), Err(Error::NotFound));
    assert_eq!(
        failed(state(&mut c, "plan.txt", How::Existing)),
        Error::NotFound
    );
    // A restart finds the same folder.
    let object = c.server.system_state;
    drop(c);
    let (c, _) = Client::start(&mut disk);
    assert_eq!(c.server.system_state, object);
}

#[test]
fn an_actions_generation_is_the_one_its_commit_gets() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let file = c.open("a.txt", How::CreateFile).unwrap();
    // Outside an action there is no commit to name.
    assert_eq!(failed(c.home(Request::Generation)), Error::Invalid);
    c.home(Request::Begin { action: 1 });
    let Reply::Generation { generation } = c.home(Request::Generation).0 else {
        panic!("no generation");
    };
    // Only the action's owner is told; Begin's lock keeps the answer true.
    assert_eq!(
        failed(c.call(BOB, HOME_USER, DIR_RIGHTS, Request::Generation)),
        Error::Busy
    );
    c.write(&file, 0, b"named");
    assert_eq!(c.home(Request::Commit).0, Reply::Committed { generation });
    assert_eq!(failed(c.home(Request::Generation)), Error::Invalid);
}

#[test]
fn a_writer_that_never_pauses_still_gets_its_undo_space_back() {
    // A small disk: without retention, every action's undo snapshot would keep
    // the blocks its overwrite replaced, and these actions would fill it many
    // times over. (Trash is kept until it is emptied, so none is made here.)
    let mut disk = MemDisk::new(2 * 1024);
    let (mut c, _) = Client::start(&mut disk);
    let data = vec![0x5a; 6 * 1024];
    for i in 0..2_000u64 {
        assert_eq!(c.home(Request::Begin { action: i + 1 }).0, Reply::Done);
        let how = if i < 4 {
            How::CreateFile
        } else {
            How::Existing
        };
        let file = c.open(&format!("f{}", i % 4), how).unwrap();
        assert_eq!(c.write(&file, 0, &data), Reply::Done);
        match c.home(Request::Commit).0 {
            Reply::Committed { .. } => {}
            other => panic!("action {i}: {other:?}"),
        }
        let now = c.now;
        c.server.idle(now);
    }
    let (free, total) = c.server.fs().space();
    assert!(free * 20 > total, "{free} of {total} blocks free");
}

/// The pid [`a_dead_owners_action_is_aborted_and_writes_go_on`] has killed.
static DEAD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

#[test]
fn a_dead_owners_action_is_aborted_and_writes_go_on() {
    use core::sync::atomic::Ordering;
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    c.server.set_alive(|pid| pid != DEAD.load(Ordering::SeqCst));
    // Alice opens an action, changes a file, and dies before Commit.
    assert_eq!(c.home(Request::Begin { action: 1 }).0, Reply::Done);
    let half = c.open("half.txt", How::CreateFile).unwrap();
    assert_eq!(c.write(&half, 0, b"half done"), Reply::Done);
    let bob_begins = |c: &mut Client| {
        c.call(BOB, HOME_USER, DIR_RIGHTS, Request::Begin { action: 2 })
            .0
    };
    // Alive, she holds the action: Bob waits.
    assert_eq!(bob_begins(&mut c), Reply::Failed(Error::Busy));
    DEAD.store(ALICE, Ordering::SeqCst);
    // Dead, her action goes, and nothing of it stays.
    assert_eq!(bob_begins(&mut c), Reply::Done);
    let found = c.call(
        BOB,
        HOME_USER,
        DIR_RIGHTS,
        Request::Open {
            path: "half.txt".into(),
            rights: Rights::READ.bits(),
            how: How::Existing,
        },
    );
    assert_eq!(failed(found), Error::NotFound);
    let committed = c.call(BOB, HOME_USER, DIR_RIGHTS, Request::Commit).0;
    assert!(
        matches!(committed, Reply::Committed { .. }),
        "{committed:?}"
    );
    // Idle reaps too: a dead owner's action never waits for the next writer.
    DEAD.store(BOB, Ordering::SeqCst);
    assert_eq!(bob_begins(&mut c), Reply::Done);
    let now = c.now;
    c.server.idle(now);
    assert_eq!(c.home(Request::Begin { action: 3 }).0, Reply::Done);
    DEAD.store(0, Ordering::SeqCst);
}

impl Client<'_> {
    /// Moves `name` in the folder `dir` names (its badge) to the trash.
    fn trash(&mut self, dir: u64, name: &str) {
        let request = Request::Trash { name: name.into() };
        assert_eq!(self.call(ALICE, dir, DIR_RIGHTS, request).0, Reply::Done);
    }

    /// The whole trash as the `Dir` `dir` sees it, page by page.
    fn trash_paths(&mut self, dir: u64) -> Vec<String> {
        let (mut paths, mut after) = (Vec::new(), None);
        loop {
            match self
                .call(ALICE, dir, DIR_RIGHTS, Request::TrashList { after })
                .0
            {
                Reply::Trash { items, more } => {
                    after = items.last().map(|i| i.id);
                    paths.extend(items.into_iter().map(|i| i.path));
                    if !more {
                        return paths;
                    }
                }
                other => panic!("{other:?}"),
            }
        }
    }
}

#[test]
fn the_trash_lists_by_path_and_gives_back_by_name() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let notes = c.open("notes", How::Mkdir).unwrap();
    let plan = c.open("notes/plan.txt", How::CreateFile).unwrap();
    c.write(&plan, 0, b"buy milk");
    c.open("old.txt", How::CreateFile).unwrap();
    c.trash(notes.badge, "plan.txt");
    c.trash(HOME_USER, "old.txt");
    assert_eq!(c.trash_paths(HOME_USER), ["notes/plan.txt", "old.txt"]);
    let Reply::Trash { items, more } = c.home(Request::TrashList { after: None }).0 else {
        panic!("no trash");
    };
    assert!(!more);
    assert_eq!(items[0].kind, fsproto::Kind::File);
    assert!(items[0].deleted > 0, "{items:?}");
    // A name taken since is not overwritten, and the item stays in the trash.
    let taken = c.open("old.txt", How::CreateFile).unwrap();
    let back = |c: &mut Client, path: &str| c.home(Request::TrashRestore { path: path.into() }).0;
    assert_eq!(back(&mut c, "old.txt"), Reply::Failed(Error::Exists));
    assert_eq!(c.trash_paths(HOME_USER).len(), 2);
    assert_eq!(c.write(&taken, 0, b"new"), Reply::Done);
    // Back by name, bytes and all; then it is no longer in the trash.
    assert_eq!(back(&mut c, "notes/plan.txt"), Reply::Done);
    let plan = c.open("notes/plan.txt", How::Existing).unwrap();
    assert_eq!(c.read(&plan), Ok(b"buy milk".to_vec()));
    assert_eq!(c.trash_paths(HOME_USER), ["old.txt"]);
    assert_eq!(
        back(&mut c, "notes/plan.txt"),
        Reply::Failed(Error::NotFound)
    );
    assert_eq!(back(&mut c, "../plan.txt"), Reply::Failed(Error::Invalid));
}

#[test]
fn a_restore_from_the_trash_is_undone_like_any_action() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    c.open("todo.txt", How::CreateFile).unwrap();
    c.trash(HOME_USER, "todo.txt");
    c.home(Request::Begin { action: 9 });
    let request = Request::TrashRestore {
        path: "todo.txt".into(),
    };
    assert_eq!(c.home(request).0, Reply::Done);
    let Reply::Committed { generation } = c.home(Request::Commit).0 else {
        panic!("not committed");
    };
    assert!(c.trash_paths(HOME_USER).is_empty());
    let undone = c
        .home(Request::Restore {
            txn: generation,
            action: 10,
        })
        .0;
    assert!(matches!(undone, Reply::Restored { .. }), "{undone:?}");
    assert_eq!(c.open("todo.txt", How::Existing), Err(Error::NotFound));
    assert_eq!(c.trash_paths(HOME_USER), ["todo.txt"]);
}

#[test]
fn emptying_reaches_only_what_was_beneath_the_dir_and_needs_delete() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let notes = c.open("notes", How::Mkdir).unwrap();
    c.open("notes/a.txt", How::CreateFile).unwrap();
    c.open("b.txt", How::CreateFile).unwrap();
    c.trash(notes.badge, "a.txt");
    c.trash(HOME_USER, "b.txt");
    // The notes Dir sees only its own, by paths relative to it.
    assert_eq!(c.trash_paths(notes.badge), ["a.txt"]);
    let reader = DIR_RIGHTS.difference(Rights::DELETE);
    let refused = c.call(ALICE, notes.badge, reader, Request::EmptyTrash);
    assert_eq!(failed(refused), Error::Permission);
    let emptied = c.call(ALICE, notes.badge, DIR_RIGHTS, Request::EmptyTrash);
    assert_eq!(emptied.0, Reply::Emptied { count: 1 });
    assert_eq!(c.trash_paths(HOME_USER), ["b.txt"]);
    assert_eq!(c.home(Request::EmptyTrash).0, Reply::Emptied { count: 1 });
    assert!(c.trash_paths(HOME_USER).is_empty());
    assert_eq!(c.home(Request::EmptyTrash).0, Reply::Emptied { count: 0 });
}

#[test]
fn a_long_trash_comes_in_pages() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let count = fsproto::MAX_TRASH_ITEMS + 5;
    for i in 0..count {
        let name = format!("f{i}");
        c.open(&name, How::CreateFile).unwrap();
        c.trash(HOME_USER, &name);
    }
    let Reply::Trash { items, more } = c.home(Request::TrashList { after: None }).0 else {
        panic!("no trash");
    };
    assert_eq!((items.len(), more), (fsproto::MAX_TRASH_ITEMS, true));
    let all = c.trash_paths(HOME_USER);
    let want: Vec<String> = (0..count).map(|i| format!("f{i}")).collect();
    assert_eq!(all, want);
}

#[test]
fn the_model_store_hands_over_a_sealed_models_runs() {
    let mut disk = MemDisk::new(BLOCKS);
    let (mut c, _) = Client::start(&mut disk);
    let sha = [0x5a; 32];
    let name: String = sha.iter().map(|b| format!("{b:02x}")).collect::<String>() + ".gguf";
    assert_eq!(c.server.model_extents(&sha), Ok(None), "no model yet");
    let len = (5 << 20) + 1234;
    let fs = &mut c.server.fs;
    fs.set_volume(MODELS.into()).unwrap();
    let mut at = 0u64;
    fs.write_sealed(ROOT, &name, len, &mut |piece: &mut [u8]| {
        for byte in piece.iter_mut() {
            *byte = (at % 251) as u8;
            at += 1;
        }
        Ok(())
    })
    .unwrap();
    fs.commit().unwrap();
    let extents = c
        .server
        .model_extents(&sha)
        .unwrap()
        .expect("the sealed model");
    let blocks: u64 = extents.iter().map(|&(_, n)| n).sum();
    assert_eq!(blocks, len.div_ceil(4096), "{extents:?}");
    assert!(extents.len() <= 3, "few runs: {extents:?}");
    // A plain file of that name is not the model.
    assert_eq!(c.server.model_extents(&[0x11; 32]), Ok(None));
}
