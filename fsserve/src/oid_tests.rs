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

/// A disk whose writes fail while `failing` is set.
struct Failing<'d> {
    disk: &'d mut MemDisk,
    failing: alloc::rc::Rc<core::cell::Cell<bool>>,
}

impl kaifs::BlockDev for Failing<'_> {
    fn info(&self) -> kaifs::DevInfo {
        self.disk.info()
    }

    fn read(&mut self, addr: u64, buf: &mut kaifs::Block) -> Result<(), kaifs::IoError> {
        self.disk.read(addr, buf)
    }

    fn write(&mut self, addr: u64, data: &kaifs::Block) -> Result<(), kaifs::IoError> {
        match self.failing.get() {
            true => Err(kaifs::IoError),
            false => self.disk.write(addr, data),
        }
    }

    fn flush(&mut self) -> Result<(), kaifs::IoError> {
        match self.failing.get() {
            true => Err(kaifs::IoError),
            false => self.disk.flush(),
        }
    }
}

fn create_on<D: kaifs::BlockDev>(server: &mut Server<D>, path: &str) -> Reply {
    let open = Request::Open {
        path: path.into(),
        rights: DIR_RIGHTS.bits(),
        how: How::CreateFile,
    };
    let bytes = fsproto::encode_request(&open);
    let answer = server.call(&Call {
        caller: BOB,
        badge: HOME_USER,
        rights: DIR_RIGHTS,
        request: &bytes,
        now: 1_000,
    });
    fsproto::decode_reply(&answer.reply).unwrap()
}

/// A disk that cannot be written at start still mounts and serves: no id
/// goes out until a commit saves the skipped counters, and once writes work
/// again the next change saves them first.
#[test]
fn fs_starts_on_a_disk_it_cannot_write_and_mints_no_id_until_it_can() {
    let mut disk = MemDisk::new(BLOCKS);
    let kept = {
        let (mut c, _) = Client::start(&mut disk);
        assert_eq!(
            home(&mut c, ALICE, Request::Begin { action: 1 }).0,
            Reply::Done
        );
        create(&mut c, ALICE, "t", How::CreateFile)
        // `fs` stops mid-action.
    };
    let failing = alloc::rc::Rc::new(core::cell::Cell::new(true));
    let dev = Failing {
        disk: &mut disk,
        failing: failing.clone(),
    };
    let (mut server, _) = Server::start(dev, [7; 16]).expect("starts on a failing disk");
    assert!(matches!(create_on(&mut server, "x"), Reply::Failed(_)));
    failing.set(false);
    assert!(matches!(create_on(&mut server, "y"), Reply::Opened(..)));
    drop(server);
    let (mut c, _) = Client::start(&mut disk);
    assert_eq!(stale(&mut c, &kept), Reply::Failed(Error::NotFound));
}
