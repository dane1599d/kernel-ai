//! Undo (ADR 0004 §4; P2 task 6): [`Fs::restore`] puts back what an action
//! changed, never over a later change; [`Fs::changes`] finds what changed since a
//! generation by reading only what changed; [`Fs::history`] lists an object's
//! versions in the snapshots.
//!
//! **`restore(txn)`'s rules**, on the active volume, from the action's UNDO
//! record and its undo snapshot, as a new transaction the caller commits (as an
//! action of its own, so a restore can itself be undone):
//! 1. An object changed after the action (its inode's `changed` is later) is never
//!    touched. A file's version from before the action is written beside it as
//!    `name (restored)`; a folder is left as it is.
//! 2. An object the action made, and nothing changed since, is removed again,
//!    deepest first; a folder that still holds names is left.
//! 3. An object the action changed or removed, and nothing changed since, gets
//!    its version from before the action back, shallowest first: its data and
//!    attributes, and its place. If its old folder is gone it goes in the root; if
//!    its old name is taken there it lands beside as `name (restored)`.
//!
//! Each object's outcome comes back, so the caller can say what happened.

use crate::Error;
use crate::dev::BlockDev;
use crate::format::BlockPtr;
use crate::fs::{Fs, decode_value, inline_of, value_len};
use crate::items::{
    Entry, FileType, Inode, K_DATA, K_DIRENT, K_INODE, K_XATTR, MAX_NAME, ROOT, decode_names,
    valid_name,
};
use crate::trash::TRASH;
use crate::tree::Key;
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// What `restore` did with one object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Its version from before the action is back in its place.
    Back,
    /// Back, but under another name or in the root: its old place was taken.
    Moved { name: String },
    /// Changed later, so left; its old version is the new file `ino`, `name`.
    Beside { ino: u64, name: String },
    /// Made by the action, and removed again.
    Removed,
    /// Left as it is: changed later, or a folder that still holds names.
    Left,
}

/// One version of an object in a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub snapshot: u64,
    pub generation: u64,
    pub inode: Inode,
}

/// The live tree, or a snapshot's tree, of the active volume.
#[derive(Clone, Copy)]
enum View {
    Live,
    Tree(BlockPtr),
}

impl<D: BlockDev> Fs<D> {
    fn raw_in(&mut self, view: View, key: &Key) -> Result<Option<Vec<u8>>, Error> {
        match view {
            View::Live => self.get_raw(key),
            View::Tree(root) => self.get_raw_in(&root, key),
        }
    }

    /// The raw items of object `oid` of kind `kind` in `view`.
    fn items_in(&mut self, view: View, oid: u64, kind: u8) -> Result<Vec<(Key, Vec<u8>)>, Error> {
        let mut out = Vec::new();
        let mut visit = |key: &Key, raw: &[u8]| {
            let mine = key.oid == oid && key.kind == kind;
            if mine {
                out.push((*key, raw.to_vec()));
            }
            mine
        };
        let from = Key::new(oid, kind, 0);
        match view {
            View::Live => self.scan_raw(&from, &mut visit)?,
            View::Tree(root) => self.scan_tree(&root, &from, &mut visit)?,
        }
        Ok(out)
    }

    fn inode_in(&mut self, view: View, ino: u64) -> Result<Option<Inode>, Error> {
        match self.raw_in(view, &Key::new(ino, K_INODE, 0))? {
            None => Ok(None),
            Some(raw) => {
                let bytes = inline_of(&raw).ok_or(Error::Corrupt("inode"))?;
                Inode::decode(bytes).map(Some)
            }
        }
    }

    /// The name `ino` has in its folder `parent` in `view`.
    fn name_in(&mut self, view: View, parent: u64, ino: u64) -> Result<Option<Entry>, Error> {
        for (_, raw) in self.items_in(view, parent, K_DIRENT)? {
            let bytes = inline_of(&raw).ok_or(Error::Corrupt("folder names"))?;
            if let Some(e) = decode_names(bytes)?.into_iter().find(|e| e.ino == ino) {
                return Ok(Some(e));
            }
        }
        Ok(None)
    }

    /// Whether `ino` is one of folders `dirs` or lies under one in `view`.
    fn under_in(&mut self, view: View, ino: u64, dirs: &[u64]) -> Result<bool, Error> {
        let mut seen = BTreeSet::new();
        let mut at = ino;
        while !dirs.contains(&at) {
            if at == ROOT {
                return Ok(false);
            }
            if !seen.insert(at) {
                return Err(Error::Corrupt("a folder's parents loop"));
            }
            at = match self.inode_in(view, at)? {
                Some(inode) => inode.parent,
                None => return Ok(false),
            };
        }
        Ok(true)
    }

    /// Whether everything the action committed as `txn` changed lies in folder
    /// `dir` (or is it) or in the trash, both before the action and live now:
    /// what [`Fs::restore`] would touch stays there. The root counts only when
    /// the action made the trash, the one change a trash makes there.
    pub fn restore_within(&mut self, txn: u64, dir: u64) -> Result<bool, Error> {
        let undo = self.undo_record(txn)?.ok_or(Error::NotFound)?;
        let snap = *self
            .snapshots()
            .get(&undo.snapshot)
            .ok_or(Error::NotFound)?;
        let then = View::Tree(snap.root);
        let trash = self.lookup(ROOT, TRASH)?.map(|e| e.ino);
        let scope = [Some(dir), trash].into_iter().flatten().collect::<Vec<_>>();
        let made_trash = trash.is_some_and(|t| undo.objects.contains(&t));
        for &ino in &undo.objects {
            if ino == ROOT && made_trash {
                continue;
            }
            for view in [then, View::Live] {
                let present = self.inode_in(view, ino)?.is_some();
                if present && !self.under_in(view, ino, &scope)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// How many folders lie between `ino` and the root in `view`.
    fn depth_in(&mut self, view: View, ino: u64) -> Result<usize, Error> {
        let mut seen = BTreeSet::new();
        let mut at = ino;
        while at != ROOT {
            if !seen.insert(at) {
                return Err(Error::Corrupt("a folder's parents loop"));
            }
            at = self
                .inode_in(view, at)?
                .ok_or(Error::Corrupt("a parent that is gone"))?
                .parent;
        }
        Ok(seen.len())
    }

    /// `ino`'s path in the active volume, live or in snapshot `snapshot`.
    pub fn path(&mut self, snapshot: Option<u64>, ino: u64) -> Result<Option<String>, Error> {
        let view = match snapshot {
            None => View::Live,
            Some(id) => View::Tree(self.snapshots().get(&id).ok_or(Error::NotFound)?.root),
        };
        let mut parts = Vec::new();
        let mut seen = BTreeSet::new();
        let mut at = ino;
        while at != ROOT {
            let Some(inode) = self.inode_in(view, at)? else {
                return Ok(None);
            };
            let Some(entry) = self.name_in(view, inode.parent, at)? else {
                return Ok(None);
            };
            if !seen.insert(at) {
                return Err(Error::Corrupt("a folder's parents loop"));
            }
            parts.push(entry.name);
            at = inode.parent;
        }
        if self.inode_in(view, ROOT)?.is_none() {
            return Ok(None);
        }
        parts.reverse();
        Ok(Some(format!("/{}", parts.join("/"))))
    }

    /// A name for an old version beside `name` in `dir` that nothing has.
    fn beside(&mut self, dir: u64, name: &str) -> Result<String, Error> {
        for n in 1..1000u32 {
            let tail = match n {
                1 => String::from(" (restored)"),
                _ => format!(" (restored {n})"),
            };
            let mut stem = name;
            while stem.len() + tail.len() > MAX_NAME {
                let mut cut = stem.len() - 1;
                while !stem.is_char_boundary(cut) {
                    cut -= 1;
                }
                stem = &stem[..cut];
            }
            let candidate = format!("{stem}{tail}");
            if valid_name(&candidate) && self.lookup(dir, &candidate)?.is_none() {
                return Ok(candidate);
            }
        }
        Err(Error::Invalid("no free name beside it"))
    }

    /// Makes `ino`'s items of kind `kind` in the live tree those of `from`'s
    /// object `src`: items that are alike are left, the rest are set or removed.
    fn copy_items(&mut self, from: View, src: u64, ino: u64, kind: u8) -> Result<(), Error> {
        let want: Vec<(u64, Vec<u8>)> = self
            .items_in(from, src, kind)?
            .into_iter()
            .map(|(k, v)| (k.off, v))
            .collect();
        let have = self.items_in(View::Live, ino, kind)?;
        for (key, raw) in &have {
            if !want.iter().any(|(off, v)| *off == key.off && v == raw) {
                self.delete(key)?;
            }
        }
        for (off, raw) in want {
            let key = Key::new(ino, kind, off);
            if !have.iter().any(|(k, v)| *k == key && *v == raw) {
                self.set(key, raw)?;
            }
        }
        Ok(())
    }

    /// Writes `old`'s data and attributes over `ino`'s, and the fields a version
    /// keeps: size, mode and times.
    fn put_back(&mut self, root: BlockPtr, ino: u64, old: &Inode) -> Result<(), Error> {
        let view = View::Tree(root);
        if old.kind == FileType::File {
            self.copy_items(view, ino, ino, K_DATA)?;
        }
        self.copy_items(view, ino, ino, K_XATTR)?;
        let mut inode = self.stat(ino)?;
        if old.kind == FileType::File {
            inode.size = old.size;
        }
        (inode.mode, inode.mtime, inode.crtime, inode.ctime) =
            (old.mode, old.mtime, old.crtime, self.clock);
        self.put_inode(ino, &inode)
    }

    /// Puts back, as rule 3 says, an object the action changed or removed.
    fn bring_back(&mut self, root: BlockPtr, ino: u64, old: &Inode) -> Result<Outcome, Error> {
        let view = View::Tree(root);
        let old_name = match ino {
            ROOT => None,
            _ => Some(
                self.name_in(view, old.parent, ino)?
                    .ok_or(Error::Corrupt("a snapshot's object without a name"))?,
            ),
        };
        let live = self.inode_in(View::Live, ino)?;
        let Some(old_name) = old_name else {
            self.put_back(root, ino, old)?;
            return Ok(Outcome::Back);
        };
        let dir = match self.inode_in(View::Live, old.parent)? {
            Some(i) if i.kind == FileType::Dir => old.parent,
            _ => ROOT,
        };
        let here = match live {
            Some(l) => self
                .name_in(View::Live, l.parent, ino)?
                .map(|e| (l.parent, e.name)),
            None => None,
        };
        let in_place = here
            .as_ref()
            .is_some_and(|(d, n)| *d == dir && *n == old_name.name);
        let name = match in_place || self.lookup(dir, &old_name.name)?.is_none() {
            true => old_name.name.clone(),
            false => self.beside(dir, &old_name.name)?,
        };
        match (live, here) {
            (Some(_), Some((at, now))) if !in_place => {
                if let Err(e) = self.rename(at, &now, dir, &name) {
                    return match e {
                        Error::Invalid(_) | Error::Exists => Ok(Outcome::Left),
                        e => Err(e),
                    };
                }
            }
            (Some(_), _) => {}
            (None, _) => {
                let inode = Inode {
                    nlink: match old.kind {
                        FileType::File => 1,
                        FileType::Dir => 2,
                    },
                    size: match old.kind {
                        FileType::File => old.size,
                        FileType::Dir => 0,
                    },
                    parent: dir,
                    ..*old
                };
                self.put_inode(ino, &inode)?;
                self.link(dir, &name, ino, old.kind)?;
            }
        }
        self.put_back(root, ino, old)?;
        Ok(match dir == old.parent && name == old_name.name {
            true => Outcome::Back,
            false => Outcome::Moved { name },
        })
    }

    /// Writes, as rule 1 says, the old version of a file changed later beside it.
    fn keep_beside(
        &mut self,
        root: BlockPtr,
        ino: u64,
        old: &Inode,
        live: &Inode,
    ) -> Result<Outcome, Error> {
        if old.kind != FileType::File || live.kind != FileType::File {
            return Ok(Outcome::Left);
        }
        let Some(old_name) = self.name_in(View::Tree(root), old.parent, ino)? else {
            return Ok(Outcome::Left);
        };
        let dir = live.parent;
        let name = self.beside(dir, &old_name.name)?;
        let copy = self.create(dir, &name)?;
        // The data is copied by value: two items of one tree never share a record,
        // or replacing either could free it under the other.
        for (key, raw) in self.items_in(View::Tree(root), ino, K_DATA)? {
            let bytes = decode_value(&raw, &mut self.io)?.into_bytes();
            self.write_at(copy, key.off, &bytes)?;
        }
        self.copy_items(View::Tree(root), ino, copy, K_XATTR)?;
        let mut inode = self.stat(copy)?;
        (inode.size, inode.mode, inode.mtime, inode.crtime) =
            (old.size, old.mode, old.mtime, old.crtime);
        self.put_inode(copy, &inode)?;
        Ok(Outcome::Beside { ino: copy, name })
    }

    /// Restores what the action committed as `txn` changed in the active volume,
    /// by the rules above. Returns each object's outcome.
    pub fn restore(&mut self, txn: u64) -> Result<Vec<(u64, Outcome)>, Error> {
        self.writing()?;
        let undo = self.undo_record(txn)?.ok_or(Error::NotFound)?;
        let snap = *self
            .snapshots()
            .get(&undo.snapshot)
            .ok_or(Error::NotFound)?;
        let (root, then) = (snap.root, View::Tree(snap.root));
        let mut out = Vec::new();
        let mut made = Vec::new();
        let mut were = Vec::new();
        for &ino in &undo.objects {
            let old = self.inode_in(then, ino)?;
            let live = self.inode_in(View::Live, ino)?;
            match (old, live) {
                (_, Some(l)) if l.changed > txn => {
                    let outcome = match old {
                        Some(o) => self.keep_beside(root, ino, &o, &l)?,
                        None => Outcome::Left,
                    };
                    out.push((ino, outcome));
                }
                (None, Some(l)) => made.push((self.depth_in(View::Live, ino)?, ino, l.kind)),
                (Some(o), _) => were.push((self.depth_in(then, ino)?, ino, o)),
                (None, None) => {}
            }
        }
        // Files the action made go first, so their names are free for what comes
        // back; folders it made go last, once what was moved into them is back.
        made.sort_by(|a, b| b.cmp(a));
        let (files, dirs): (Vec<_>, Vec<_>) = made
            .into_iter()
            .partition(|(_, _, kind)| *kind == FileType::File);
        for (_, ino, _) in files {
            out.push((ino, self.unmake(ino)?));
        }
        were.sort_by_key(|(depth, ino, _)| (*depth, *ino));
        for (_, ino, old) in were {
            let outcome = self.bring_back(root, ino, &old)?;
            out.push((ino, outcome));
        }
        for (_, ino, _) in dirs {
            out.push((ino, self.unmake(ino)?));
        }
        Ok(out)
    }

    /// Removes, as rule 2 says, an object the action made: a folder only once it
    /// is empty.
    fn unmake(&mut self, ino: u64) -> Result<Outcome, Error> {
        let live = self.stat(ino)?;
        Ok(match self.name_in(View::Live, live.parent, ino)? {
            Some(_) if live.kind == FileType::Dir && live.size > 0 => Outcome::Left,
            Some(e) => {
                self.remove(live.parent, &e.name)?;
                Outcome::Removed
            }
            None => Outcome::Left,
        })
    }

    /// The objects of the active volume whose inodes changed after generation
    /// `since`, with their inodes. It reads only the subtrees born after `since`,
    /// so its cost follows what changed, not the volume's size. An object removed
    /// since has no inode, so the UNDO records name it instead.
    pub fn changes(&mut self, since: u64) -> Result<Vec<(u64, Inode)>, Error> {
        let mut out = Vec::new();
        let mut failed = None;
        self.scan_born_after(since, &mut |key, raw| {
            if key.kind != K_INODE || key.off != 0 {
                return;
            }
            match inline_of(raw).map(Inode::decode) {
                Some(Ok(inode)) if inode.changed > since => out.push((key.oid, inode)),
                Some(Ok(_)) => {}
                _ => failed = Some(Error::Corrupt("inode")),
            }
        })?;
        match failed {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }

    /// The versions of object `ino` in the active volume's snapshots, newest
    /// first, each once: a snapshot holding the same version as a newer one, or
    /// as the live tree, is left out.
    pub fn history(&mut self, ino: u64) -> Result<Vec<Version>, Error> {
        let snaps: Vec<(u64, u64, BlockPtr)> = self
            .snapshots()
            .iter()
            .rev()
            .map(|(id, s)| (*id, s.generation, s.root))
            .collect();
        let mut out: Vec<Version> = Vec::new();
        let mut newer = self.inode_in(View::Live, ino)?.map(|i| i.changed);
        for (snapshot, generation, root) in snaps {
            let Some(inode) = self.inode_in(View::Tree(root), ino)? else {
                continue;
            };
            if inode.changed != 0 && newer == Some(inode.changed) {
                continue;
            }
            newer = Some(inode.changed);
            out.push(Version {
                snapshot,
                generation,
                inode,
            });
        }
        Ok(out)
    }

    /// Up to `len` bytes of file `ino` from `off` as snapshot `snapshot` holds it,
    /// holes as zeros.
    pub fn read_version(
        &mut self,
        snapshot: u64,
        ino: u64,
        off: u64,
        len: usize,
    ) -> Result<Vec<u8>, Error> {
        let root = self.snapshots().get(&snapshot).ok_or(Error::NotFound)?.root;
        let inode = self
            .inode_in(View::Tree(root), ino)?
            .ok_or(Error::NotFound)?;
        if inode.kind != FileType::File {
            return Err(Error::IsDir);
        }
        let end = off.saturating_add(len as u64).min(inode.size);
        if off >= end {
            return Ok(Vec::new());
        }
        let mut out = vec![0; (end - off) as usize];
        for (key, raw) in self.items_in(View::Tree(root), ino, K_DATA)? {
            let at = key.off;
            let len = value_len(&raw).ok_or(Error::Corrupt("a data item"))? as u64;
            if at.saturating_add(len) <= off || at >= end {
                continue;
            }
            let value = decode_value(&raw, &mut self.io)?;
            let bytes = value.bytes();
            let (lo, hi) = (off.max(at), end.min(at.saturating_add(bytes.len() as u64)));
            if lo < hi {
                out[(lo - off) as usize..(hi - off) as usize]
                    .copy_from_slice(&bytes[(lo - at) as usize..(hi - at) as usize]);
            }
        }
        Ok(out)
    }
}
