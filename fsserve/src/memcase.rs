//! The `memory-volume` case's two steps (agents-os.md M1), which only the
//! kernel reaches, through `fs`'s plain route: no badge names the `memory`
//! volume, so no client could run them.
//!
//! - [`Server::memory_probe`] (boot 1): a record made in `memory` in one
//!   action, a named snapshot over it, the record forgotten in a second action,
//!   then purged: every snapshot that held it goes.
//! - [`Server::memory_check`] (boot 2, same disk): no snapshot of `memory`
//!   holds any object but its root folder.

use super::{Server, error};
use alloc::vec::Vec;
use fsproto::Error;
use kaifs::items::K_INODE;
use kaifs::tree::Key;
use kaifs::vol::MEMORY;
use kaifs::{BlockDev, ROOT};

/// The probe's actions: far from the shell's, whose ids count up from 1.
const PROBE_ACTIONS: [u64; 2] = [u64::MAX - 1, u64::MAX];
const RECORD: &str = "probe";

impl<D: BlockDev> Server<D> {
    /// Boot 1: how many snapshots the purge dropped. Refused while a client
    /// holds an action, so nothing of theirs is in its commits.
    pub fn memory_probe(&mut self) -> Result<usize, Error> {
        if self.owner.is_some() || self.pending.is_some() {
            return Err(Error::Busy);
        }
        let fs = self.fs();
        let active = fs.volume();
        let probed = (|| {
            fs.set_volume(MEMORY)?;
            fs.begin(PROBE_ACTIONS[0])?;
            let record = fs.create(ROOT, RECORD)?;
            fs.write_file(record, b"a fact the user asked to forget")?;
            fs.commit()?;
            fs.snapshot()?;
            fs.commit()?;
            fs.begin(PROBE_ACTIONS[1])?;
            fs.remove(ROOT, RECORD)?;
            fs.commit()?;
            let dropped = fs.purge(record)?;
            fs.commit()?;
            Ok(dropped)
        })();
        let _ = fs.set_volume(active);
        probed.map_err(error)
    }

    /// Boot 2: `memory`'s snapshots, and how many hold an object other than
    /// its root folder.
    pub fn memory_check(&mut self) -> Result<(usize, usize), Error> {
        let fs = self.fs();
        let active = fs.volume();
        let checked = (|| {
            fs.set_volume(MEMORY)?;
            let roots: Vec<_> = fs.snapshots().values().map(|s| s.root).collect();
            let mut holding = 0;
            for root in &roots {
                let mut found = false;
                let mut visit = |key: &Key, _: &[u8]| {
                    found = key.kind == K_INODE && key.oid != ROOT;
                    !found
                };
                fs.scan_tree(root, &Key::new(ROOT + 1, K_INODE, 0), &mut visit)?;
                holding += usize::from(found);
            }
            Ok((roots.len(), holding))
        })();
        let _ = fs.set_volume(active);
        checked.map_err(error)
    }
}
