//! [`Request::Changes`]: what changed beneath a handle's folder since a
//! generation, a page at a time.
//!
//! - A round lists the committed changes in `(since, upto]`, `upto` being
//!   the committed generation at its first page, in object order: each page
//!   goes on after `(upto, last object)`. A change made during the round
//!   (an open action's too, once it commits) has a generation past `upto`,
//!   so the next round, from `upto`, gives it: each object at least once,
//!   never missed, and each page reads about a page of the tree.
//! - [`fsproto::Reply::ChangesPage`] says the generation the listing is
//!   complete up to and how many mounts fell back: a reader that sees that
//!   count change reads everything again, since a fallback undoes commits.
//! - A handle on a volume's root sees the whole volume; any other only what
//!   lies beneath its folder in the live tree, so a folder's holder learns
//!   nothing of the objects outside it.

use super::{Server, error, stat};
use alloc::vec::Vec;
use fsproto::{Error, MAX_ROWS, Stat};
use kaifs::fs::inline_of;
use kaifs::items::K_INODE;
use kaifs::{BlockDev, Inode, Key, ROOT};

/// Inodes gathered between the scope checks, which need the tree the scan holds.
const BATCH: usize = 4 * MAX_ROWS as usize;

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

    /// A page of the objects beneath `dir` whose last committed change is in
    /// `(since, upto]`, in object order after the cursor, `upto` being the
    /// round's ceiling (the first page's committed generation, carried back in
    /// `after`); whether more follow, and `upto`. Each page reads about a
    /// page's worth of the tree ([`kaifs::Fs::scan_born_after_from`]).
    pub(crate) fn changed_since(
        &mut self,
        dir: u64,
        since: u64,
        after: Option<(u64, u64)>,
    ) -> Result<(Vec<Stat>, bool, u64), Error> {
        let fs = self.fs();
        let upto = after.map_or(fs.committed, |(ceiling, _)| ceiling.min(fs.committed));
        let mut from = match after {
            Some((_, object)) => match object.checked_add(1) {
                Some(next) => Key::new(next, 0, 0),
                None => return Ok((Vec::new(), false, upto)),
            },
            None => Key::MIN,
        };
        let mut rows = Vec::new();
        loop {
            let mut batch: Vec<(u64, Inode)> = Vec::new();
            let mut last = None;
            let mut bad = false;
            fs.scan_born_after_from(since, &from, &mut |key, raw| {
                last = Some(*key);
                if key.kind == K_INODE && key.off == 0 {
                    match inline_of(raw).map(Inode::decode) {
                        // An open action's changes carry the generation it
                        // will commit as: past `upto`, so the next round.
                        Some(Ok(inode)) if inode.changed > since && inode.changed <= upto => {
                            batch.push((key.oid, inode))
                        }
                        Some(Ok(_)) => {}
                        _ => bad = true,
                    }
                }
                !bad && batch.len() < BATCH
            })
            .map_err(error)?;
            if bad {
                return Err(Error::Io);
            }
            let ended = batch.len() < BATCH;
            for (ino, inode) in batch {
                if dir != ROOT && !fs.is_beneath(ino, dir).map_err(error)? {
                    continue;
                }
                if rows.len() == MAX_ROWS as usize {
                    return Ok((rows, true, upto));
                }
                rows.push(stat(ino, &inode));
            }
            match last.and_then(|key| key.next()) {
                Some(next) if !ended => from = next,
                _ => return Ok((rows, false, upto)),
            }
        }
    }
}
