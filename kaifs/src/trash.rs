//! The trash (ADR 0004 §4; P2 task 6): deleting moves an object to
//! `/.trash/<txn>/<n>/` in its volume, beside an `info.cbor` that says where it
//! was, when, and which action moved it (the freedesktop Trash 1.0 idea). A trash
//! is a rename, so undoing the action that trashed something moves it back. An
//! object itself named `info.cbor` is held as [`ITEM`], so a slot's `info.cbor`
//! is always the one the trash wrote; its path says what it was called.
//! Only [`Fs::empty_trash`] removes anything for good, and its caller logs what
//! went (the space comes back once the undo snapshots that hold it expire and
//! the GC runs).
//!
//! `info.cbor` is `[schema, {"path": text, "deleted": uint, "action": uint}]`,
//! schema 1; a reader skips keys it does not know.

use crate::Error;
use crate::cbor::{self, Reader};
use crate::dev::BlockDev;
use crate::fs::Fs;
use crate::items::{FileType, ROOT};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// The trash folder at each volume's root.
pub const TRASH: &str = ".trash";
/// The file beside each trashed object.
pub const INFO: &str = "info.cbor";
/// The name an object called [`INFO`] has in its slot.
pub const ITEM: &str = "item";
const INFO_SCHEMA: u64 = 1;

/// What `info.cbor` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashInfo {
    /// Where the object was, in its volume.
    pub path: String,
    /// When it was trashed, by the writer's clock.
    pub deleted: u64,
    /// The action that trashed it; 0 if none.
    pub action: u64,
}

/// One trashed object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trashed {
    /// Its folder, `/.trash/<txn>/<n>`.
    pub folder: u64,
    pub ino: u64,
    /// Its name in the folder: its own, or [`ITEM`] for one called [`INFO`].
    pub name: String,
    pub info: TrashInfo,
}

pub fn encode_info(info: &TrashInfo) -> Vec<u8> {
    let mut out = Vec::new();
    cbor::array(&mut out, 2);
    cbor::uint(&mut out, INFO_SCHEMA);
    cbor::map(&mut out, 3);
    cbor::text(&mut out, "path");
    cbor::text(&mut out, &info.path);
    cbor::text(&mut out, "deleted");
    cbor::uint(&mut out, info.deleted);
    cbor::text(&mut out, "action");
    cbor::uint(&mut out, info.action);
    out
}

/// Reads `info.cbor`: a schema 1 array whose map has at least `path`, `deleted`
/// and `action`; other keys are skipped.
pub fn decode_info(bytes: &[u8]) -> Result<TrashInfo, Error> {
    let bad = Error::Corrupt("info.cbor");
    let mut r = Reader::new(bytes);
    if r.array()? != 2 || r.uint()? != INFO_SCHEMA {
        return Err(bad);
    }
    let (mut path, mut deleted, mut action) = (None, None, None);
    for _ in 0..r.map()? {
        match r.text()?.as_str() {
            "path" => path = Some(r.text()?),
            "deleted" => deleted = Some(r.uint()?),
            "action" => action = Some(r.uint()?),
            _ => r.skip()?,
        }
    }
    if !r.is_empty() {
        return Err(bad);
    }
    Ok(TrashInfo {
        path: path.ok_or(bad)?,
        deleted: deleted.ok_or(bad)?,
        action: action.ok_or(bad)?,
    })
}

impl<D: BlockDev> Fs<D> {
    /// The folder `name` in `dir`, made if missing.
    fn folder(&mut self, dir: u64, name: &str) -> Result<u64, Error> {
        match self.lookup(dir, name)? {
            Some(e) if e.kind == FileType::Dir => Ok(e.ino),
            Some(_) => Err(Error::Invalid("a file where the trash's folder goes")),
            None => self.mkdir(dir, name),
        }
    }

    /// Moves the name `name` in `dir` of the active volume to its trash, with an
    /// `info.cbor` beside it. The record is written first and the move comes
    /// last, so a trash that fails leaves the object where it was and at most a
    /// slot with no object, which [`Fs::trash_list`] skips.
    pub fn trash(&mut self, dir: u64, name: &str) -> Result<Trashed, Error> {
        self.writing()?;
        let entry = self.lookup(dir, name)?.ok_or(Error::NotFound)?;
        let path = self
            .path(None, entry.ino)?
            .ok_or(Error::Corrupt("an object without a path"))?;
        if path == format!("/{TRASH}") || path.starts_with(&format!("/{TRASH}/")) {
            return Err(Error::Invalid("the trash itself, or what it holds"));
        }
        let trash = self.folder(ROOT, TRASH)?;
        let txn = self.folder(trash, &format!("{}", self.io.txn))?;
        let mut n = 1u64;
        while self.lookup(txn, &format!("{n}"))?.is_some() {
            n += 1;
        }
        let folder = self.mkdir(txn, &format!("{n}"))?;
        let info = TrashInfo {
            path,
            deleted: self.clock,
            action: self.action().map_or(0, |a| a.id),
        };
        let file = self.create(folder, INFO)?;
        self.write_file(file, &encode_info(&info))?;
        let held = match name {
            INFO => ITEM,
            _ => name,
        };
        self.rename(dir, name, folder, held)?;
        Ok(Trashed {
            folder,
            ino: entry.ino,
            name: held.to_string(),
            info,
        })
    }

    /// What the active volume's trash holds, by transaction then number.
    pub fn trash_list(&mut self) -> Result<Vec<Trashed>, Error> {
        let Some(trash) = self.lookup(ROOT, TRASH)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        let mut txns = self.list(trash.ino, None, usize::MAX)?;
        txns.sort_by_key(|l| l.entry.name.parse::<u64>().unwrap_or(u64::MAX));
        for txn in txns.into_iter().filter(|l| l.entry.kind == FileType::Dir) {
            let mut items = self.list(txn.entry.ino, None, usize::MAX)?;
            items.sort_by_key(|l| l.entry.name.parse::<u64>().unwrap_or(u64::MAX));
            for item in items.into_iter().filter(|l| l.entry.kind == FileType::Dir) {
                let folder = item.entry.ino;
                let inside = self.list(folder, None, usize::MAX)?;
                let info = inside
                    .iter()
                    .find(|l| l.entry.name == INFO)
                    .map(|l| l.entry.ino);
                let object = inside.iter().find(|l| l.entry.name != INFO).cloned();
                let (Some(info), Some(object)) = (info, object) else {
                    continue;
                };
                let info = decode_info(&self.read_file(info)?)?;
                out.push(Trashed {
                    folder,
                    ino: object.entry.ino,
                    name: object.entry.name,
                    info,
                });
            }
        }
        Ok(out)
    }

    /// Removes everything in the active volume's trash for good. Returns what was
    /// trashed there, for the log.
    pub fn empty_trash(&mut self) -> Result<Vec<Trashed>, Error> {
        self.writing()?;
        let gone = self.trash_list()?;
        if self.lookup(ROOT, TRASH)?.is_some() {
            self.remove_tree(ROOT, TRASH)?;
        }
        Ok(gone)
    }

    /// Removes the name `name` in `dir` and everything below it, deepest first,
    /// without recursing, so a deep tree cannot run the stack out.
    pub fn remove_tree(&mut self, dir: u64, name: &str) -> Result<(), Error> {
        self.writing()?;
        let mut stack = vec![(dir, name.to_string())];
        while let Some((at, name)) = stack.last().cloned() {
            let entry = self.lookup(at, &name)?.ok_or(Error::NotFound)?;
            if entry.kind == FileType::Dir
                && let Some(child) = self.list(entry.ino, None, 1)?.into_iter().next()
            {
                stack.push((entry.ino, child.entry.name));
                continue;
            }
            self.remove(at, &name)?;
            stack.pop();
        }
        Ok(())
    }
}
