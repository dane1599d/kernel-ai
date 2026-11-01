//! Undo kept to one folder (ADR 0004 §4): [`Fs::restore_within`] restores an
//! action only when everything the action changed, and everything the restore
//! itself changes, lies in the folder a handle was given. A trashed item counts
//! as in the folder its `info.cbor` says it came from; the trash's own folders,
//! `/.trash` and `/.trash/<txn>`, count as structure.

use crate::Error;
use crate::dev::BlockDev;
use crate::fs::{Fs, inline_of};
use crate::items::{K_DIRENT, ROOT, decode_names};
use crate::trash::{INFO, TRASH, decode_info};
use crate::undo::{Outcome, View};
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// A tree of the active volume a scope is checked in, and the snapshot that
/// names it (none for the live tree).
#[derive(Clone, Copy)]
struct At {
    view: View,
    snapshot: Option<u64>,
}

const LIVE: At = At {
    view: View::Live,
    snapshot: None,
};

impl<D: BlockDev> Fs<D> {
    /// Restores action `txn` as [`Fs::restore`] does, in the open action, if
    /// everything it changed lies in folder `dir`, both before the action and
    /// live now, and so does everything the restore changes; else `None`, and
    /// the caller drops the open transaction, as after an error. The root
    /// counts only for an action that made the trash, the one change a trash
    /// makes there.
    pub fn restore_within(
        &mut self,
        txn: u64,
        dir: u64,
    ) -> Result<Option<Vec<(u64, Outcome)>>, Error> {
        if self.action.is_none() {
            return Err(Error::Invalid("a scoped restore outside an action"));
        }
        let undo = self.undo_record(txn)?.ok_or(Error::NotFound)?;
        let snap = *self
            .snapshots()
            .get(&undo.snapshot)
            .ok_or(Error::NotFound)?;
        let then = At {
            view: View::Tree(snap.root),
            snapshot: Some(undo.snapshot),
        };
        let made_trash = match self.child_in(View::Live, ROOT, TRASH)? {
            Some(t) => undo.objects.contains(&t) && self.inode_in(then.view, t)?.is_none(),
            None => false,
        };
        for &ino in &undo.objects {
            if ino == ROOT && made_trash {
                continue;
            }
            for at in [then, LIVE] {
                if !self.in_scope(at, ino, dir)? {
                    return Ok(None);
                }
            }
        }
        let rows = self.restore(txn)?;
        // Where the restore left what it changed: an object that comes back
        // goes to its old folder's place now, or the root, which the record
        // alone does not say.
        let touched: Vec<u64> = self.vol().touched.iter().copied().collect();
        for ino in touched {
            if ino == ROOT && made_trash {
                continue;
            }
            if !self.in_scope(LIVE, ino, dir)? {
                return Ok(None);
            }
        }
        Ok(Some(rows))
    }

    /// Whether `ino` in `at` is `dir` or beneath it, or in a trash slot whose
    /// item came from beneath it; true when `at` has no `ino`.
    fn in_scope(&mut self, at: At, ino: u64, dir: u64) -> Result<bool, Error> {
        if self.inode_in(at.view, ino)?.is_none() {
            return Ok(true);
        }
        let trash = self.child_in(at.view, ROOT, TRASH)?;
        // The folders walked through, from `ino` up.
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        let mut node = ino;
        loop {
            if node == dir {
                return Ok(true);
            }
            if Some(node) == trash {
                // The chain ends with the slot and `<txn>`; shorter, `ino` is
                // the trash's own structure.
                return match chain.len().checked_sub(2).map(|i| chain[i]) {
                    Some(slot) => self.slot_beneath(at, slot, dir),
                    None => Ok(true),
                };
            }
            if node == ROOT {
                return Ok(false);
            }
            if !seen.insert(node) {
                return Err(Error::Corrupt("a folder's parents loop"));
            }
            chain.push(node);
            node = match self.inode_in(at.view, node)? {
                Some(inode) => inode.parent,
                None => return Ok(false),
            };
        }
    }

    /// Whether trash slot `slot`'s item came from beneath `dir`, by the slot's
    /// `info.cbor` and `dir`'s path, both in `at`.
    fn slot_beneath(&mut self, at: At, slot: u64, dir: u64) -> Result<bool, Error> {
        let Some(info) = self.child_in(at.view, slot, INFO)? else {
            return Ok(false);
        };
        let bytes = match at.snapshot {
            None => self.read_file(info)?,
            Some(id) => {
                let size = self.inode_in(at.view, info)?.map_or(0, |i| i.size);
                self.read_version(id, info, 0, size as usize)?
            }
        };
        let Ok(info) = decode_info(&bytes) else {
            return Ok(false);
        };
        let Some(base) = self.path(at.snapshot, dir)? else {
            return Ok(false);
        };
        let prefix = match base.as_str() {
            "/" => String::from("/"),
            _ => format!("{base}/"),
        };
        Ok(info.path.len() > prefix.len() && info.path.starts_with(&prefix))
    }

    /// The object named `name` in folder `dir` in `view`.
    fn child_in(&mut self, view: View, dir: u64, name: &str) -> Result<Option<u64>, Error> {
        for (_, raw) in self.items_in(view, dir, K_DIRENT)? {
            let bytes = inline_of(&raw).ok_or(Error::Corrupt("folder names"))?;
            if let Some(e) = decode_names(bytes)?.into_iter().find(|e| e.name == name) {
                return Ok(Some(e.ino));
            }
        }
        Ok(None)
    }
}
