//! "Done" means on the disk, even when the disk fails mid-commit: `fs`'s own
//! stack (kaifs's block cache over `Batched`) is run over a disk that fails
//! from a chosen write on, at every point of a commit in turn, and whenever a
//! change is answered Done the disk itself, mounted bare, holds it. The cache
//! once answered a failed commit's read-back from its own copy of a
//! superblock the disk never took (audit items 3 and 11).

use super::batch::{Batched, RunDev};
use super::*;
use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use kaifs::cache::Cached;
use kaifs::{BLOCK, Block, DevInfo, IoError};

const BLOCKS: usize = 4096;
/// What the change writes: one message's worth.
const BYTES: usize = fsproto::MAX_DATA;

/// The disk's bytes, its write calls so far, and the call it fails from.
struct State {
    bytes: Vec<u8>,
    runs: usize,
    fail_from: Option<usize>,
}

#[derive(Clone)]
struct Shared(Rc<RefCell<State>>);

impl Shared {
    fn failing(&self) -> bool {
        let s = self.0.borrow();
        s.fail_from.is_some_and(|n| s.runs >= n)
    }
}

impl RunDev for Shared {
    fn info(&self) -> DevInfo {
        DevInfo {
            blocks: BLOCKS as u64,
            volatile_cache: true,
            flush: true,
        }
    }

    fn read_run(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let from = addr as usize * BLOCK;
        buf.copy_from_slice(&self.0.borrow().bytes[from..from + buf.len()]);
        Ok(())
    }

    fn write_run(&mut self, addr: u64, data: &[u8]) -> Result<(), IoError> {
        if self.failing() {
            return Err(IoError);
        }
        let mut s = self.0.borrow_mut();
        s.runs += 1;
        let from = addr as usize * BLOCK;
        s.bytes[from..from + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        match self.failing() {
            true => Err(IoError),
            false => Ok(()),
        }
    }
}

/// The same bytes as a bare disk, for a mount that sees only what is there.
struct Bare(Vec<u8>);

impl kaifs::BlockDev for Bare {
    fn info(&self) -> DevInfo {
        DevInfo {
            blocks: BLOCKS as u64,
            volatile_cache: false,
            flush: true,
        }
    }

    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        let from = addr as usize * BLOCK;
        buf.copy_from_slice(&self.0[from..from + BLOCK]);
        Ok(())
    }

    fn write(&mut self, addr: u64, data: &Block) -> Result<(), IoError> {
        let from = addr as usize * BLOCK;
        self.0[from..from + BLOCK].copy_from_slice(data);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        Ok(())
    }
}

fn call<D: kaifs::BlockDev>(server: &mut Server<D>, badge: u64, request: Request) -> Answer {
    let bytes = fsproto::encode_request(&request);
    server.call(&Call {
        caller: 7,
        badge,
        rights: DIR_RIGHTS,
        request: &bytes,
        now: 1_000,
    })
}

#[test]
fn done_always_means_on_the_disk_whichever_write_fails() {
    let mut answered_done = 0;
    for fail_at in 0..40 {
        let shared = Shared(Rc::new(RefCell::new(State {
            bytes: vec![0; BLOCKS * BLOCK],
            runs: 0,
            fail_from: None,
        })));
        let stack = Cached::new(Batched::new(shared.clone()), 256);
        let (mut server, _) = Server::start(stack, [5; 16]).unwrap();
        let open = Request::Open {
            path: "f".into(),
            rights: DIR_RIGHTS.bits(),
            how: How::CreateFile,
        };
        let file = call(&mut server, HOME_USER, open).mint.expect("a file");
        // From the `fail_at`th write call on, the disk takes nothing.
        {
            let mut s = shared.0.borrow_mut();
            s.fail_from = Some(s.runs + fail_at);
        }
        let write = Request::Write {
            off: 0,
            data: vec![9; BYTES],
        };
        let answer = call(&mut server, file.badge, write);
        let reply = fsproto::decode_reply(&answer.reply).unwrap();
        let claimed = server.generation();
        drop(server);
        if reply != Reply::Done {
            continue;
        }
        answered_done += 1;
        let bytes = shared.0.borrow().bytes.clone();
        let mut fs = kaifs::Fs::mount(Bare(bytes)).expect("the disk mounts");
        assert!(
            fs.committed >= claimed,
            "failing from write {fail_at}: answered Done at {claimed}, the disk holds {}",
            fs.committed
        );
        fs.set_volume(kaifs::vol::HOME).unwrap();
        let ino = Badge::parse(file.badge).object;
        assert_eq!(fs.read_file(ino).unwrap(), vec![9; BYTES]);
    }
    assert!(answered_done > 0, "no failure point late enough to land");
}

/// A GC pass whose closing commit fails loses what it freed with that commit,
/// so the next idle runs another pass, which commits (bug hunt 5, H5-7).
/// Before, nothing asked for one, and what the pass freed stayed in use.
#[test]
fn a_pass_whose_commit_fails_runs_again() {
    let shared = Shared(Rc::new(RefCell::new(State {
        bytes: vec![0; BLOCKS * BLOCK],
        runs: 0,
        fail_from: None,
    })));
    let stack = Cached::new(Batched::new(shared.clone()), 256);
    let (mut server, _) = Server::start(stack, [5; 16]).unwrap();
    server.fs.gc_start().unwrap();
    let before = server.generation();
    // The disk takes nothing: the pass ends, and its commit fails.
    {
        let mut s = shared.0.borrow_mut();
        s.fail_from = Some(s.runs);
    }
    server.idle(1_000);
    assert!(!server.fs.gc_running());
    assert_eq!(server.generation(), before);
    assert!(server.gc_again, "nothing asks for the next pass");
    shared.0.borrow_mut().fail_from = None;
    for now in 2_000..2_100 {
        server.idle(now);
        if !server.fs.gc_running() {
            break;
        }
    }
    assert!(!server.fs.gc_running(), "the next pass never ended");
    assert!(!server.gc_again);
    assert!(
        server.generation() > before,
        "no pass ran after the failed one"
    );
}
