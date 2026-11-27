//! A disk that dies mid-change: the changes are dropped, and the abort that
//! re-reads the disk fails too. `fs` exits (its supervisor restarts it), and
//! the panic says why the changes were dropped, not only that the re-read
//! failed: the first error is the one that names the cause.

use super::batch::{Batched, RunDev};
use super::*;
use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use kaifs::cache::Cached;
use kaifs::{BLOCK, DevInfo, IoError};

const BLOCKS: usize = 4096;

/// A disk in memory that takes and gives nothing once `dead` is set.
#[derive(Clone)]
struct Dying {
    bytes: Rc<RefCell<Vec<u8>>>,
    dead: Rc<Cell<bool>>,
}

impl RunDev for Dying {
    fn info(&self) -> DevInfo {
        DevInfo {
            blocks: BLOCKS as u64,
            volatile_cache: true,
            flush: true,
        }
    }

    fn read_run(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), IoError> {
        if self.dead.get() {
            return Err(IoError);
        }
        let from = addr as usize * BLOCK;
        buf.copy_from_slice(&self.bytes.borrow()[from..from + buf.len()]);
        Ok(())
    }

    fn write_run(&mut self, addr: u64, data: &[u8]) -> Result<(), IoError> {
        if self.dead.get() {
            return Err(IoError);
        }
        let from = addr as usize * BLOCK;
        self.bytes.borrow_mut()[from..from + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        match self.dead.get() {
            true => Err(IoError),
            false => Ok(()),
        }
    }
}

#[test]
#[should_panic(expected = "the last commit reads back: Io, after Io")]
fn a_dead_disk_names_the_first_error_in_the_panic() {
    let dying = Dying {
        bytes: Rc::new(RefCell::new(vec![0; BLOCKS * BLOCK])),
        dead: Rc::new(Cell::new(false)),
    };
    let stack = Cached::new(Batched::new(dying.clone()), 256);
    let (mut server, _) = Server::start(stack, [5; 16]).unwrap();
    let open = Request::Open {
        path: "f".into(),
        rights: DIR_RIGHTS.bits(),
        how: How::CreateFile,
    };
    let bytes = fsproto::encode_request(&open);
    dying.dead.set(true);
    // The create's commit fails, and the abort cannot read the disk either.
    server.call(&Call {
        caller: 7,
        badge: HOME_USER,
        rights: DIR_RIGHTS,
        request: &bytes,
        now: 1_000,
    });
}
