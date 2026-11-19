//! [`Request::Changes`]: what changed beneath a handle's folder since a
//! generation, a page at a time.
//!
//! - Rows go in `(changed, object)` order and a page goes on after the last
//!   row's pair, so paging finds each object once (or again, later, if it
//!   changes meanwhile), however many share one generation.
//! - A handle on a volume's root sees the whole volume; any other only what
//!   lies beneath its folder in the live tree, so a folder's holder learns
//!   nothing of the objects outside it.

use super::{Server, error, stat};
use fsproto::{Error, MAX_ROWS, Reply};
use kaifs::{BlockDev, ROOT};

impl<D: BlockDev> Server<D> {
    /// The page of objects beneath `dir` changed since `since`, after `after`.
    pub(crate) fn changed_since(
        &mut self,
        dir: u64,
        since: u64,
        after: Option<(u64, u64)>,
    ) -> Result<Reply, Error> {
        let fs = self.fs();
        let mut changed = fs.changes(since).map_err(error)?;
        if dir != ROOT {
            let mut kept = alloc::vec::Vec::with_capacity(changed.len());
            for (ino, inode) in changed {
                if fs.is_beneath(ino, dir).map_err(error)? {
                    kept.push((ino, inode));
                }
            }
            changed = kept;
        }
        changed.sort_unstable_by_key(|(ino, inode)| (inode.changed, *ino));
        let start = match after {
            Some(cursor) => changed.partition_point(|(ino, inode)| (inode.changed, *ino) <= cursor),
            None => 0,
        };
        let page = &changed[start..];
        let more = page.len() > MAX_ROWS as usize;
        let rows = page
            .iter()
            .take(MAX_ROWS as usize)
            .map(|(ino, inode)| stat(*ino, inode))
            .collect();
        Ok(Reply::Changes { rows, more })
    }
}
