//! [`Request::Changes`]: what changed beneath a handle's folder since a
//! generation, a page at a time.
//!
//! - Only committed changes, in `(changed, object)` order, and a page goes on
//!   after the last row's pair, so paging finds each object once (or again,
//!   later, if it changes meanwhile), however many share one generation, and
//!   an open action's changes show after it commits.
//! - [`fsproto::Reply::ChangesPage`] says the generation the listing is
//!   complete up to and how many mounts fell back: a reader that sees that
//!   count change reads everything again, since a fallback undoes commits.
//! - A handle on a volume's root sees the whole volume; any other only what
//!   lies beneath its folder in the live tree, so a folder's holder learns
//!   nothing of the objects outside it.

use super::{Server, error, stat};
use alloc::vec::Vec;
use fsproto::{Error, MAX_ROWS, Stat};
use kaifs::{BlockDev, ROOT};

impl<D: BlockDev> Server<D> {
    /// Commits a raised fallback count before a reader sees it, so a later
    /// fallback can never show the same count again: Busy while an action
    /// holds `fs` (ask again), or the commit's error.
    pub(crate) fn save_fallbacks(&mut self) -> Result<(), Error> {
        if !self.fs.fallbacks_unsaved() {
            return Ok(());
        }
        if self.owner.is_some() || !self.fs.is_clean() {
            return Err(Error::Busy);
        }
        self.commit().map(|_| ())
    }

    /// The page of objects beneath `dir` whose last committed change is after
    /// `since`, after `after`; and whether more follow.
    pub(crate) fn changed_since(
        &mut self,
        dir: u64,
        since: u64,
        after: Option<(u64, u64)>,
    ) -> Result<(Vec<Stat>, bool), Error> {
        let fs = self.fs();
        let committed = fs.committed;
        let mut changed = fs.changes(since).map_err(error)?;
        // An open action's changes carry the generation it will commit as:
        // they show once it has, after any cursor a reader holds now.
        changed.retain(|(_, inode)| inode.changed <= committed);
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
        Ok((rows, more))
    }
}
