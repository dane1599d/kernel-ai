//! The trash through `kai-fs 1` (ADR 0004 §4): [`Request::TrashList`],
//! [`Request::TrashRestore`] and [`Request::EmptyTrash`]. kaifs keeps one trash
//! a volume, at `/.trash/<txn>/<n>/`; a `Dir` reaches only the items that were
//! beneath it, named by their paths relative to it, so the trash never shows a
//! client more than its `Dir` already could.
//!
//! [`Request::TrashList`]: fsproto::Request::TrashList
//! [`Request::TrashRestore`]: fsproto::Request::TrashRestore
//! [`Request::EmptyTrash`]: fsproto::Request::EmptyTrash

use super::{Server, error, kind};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use fsproto::{Error, MAX_TRASH_ITEMS, MAX_TRASH_TEXT, Reply, TrashItem};
use kaifs::trash::{INFO, TRASH, Trashed};
use kaifs::{BlockDev, ROOT};

impl<D: BlockDev> Server<D> {
    /// [`Request::TrashList`](fsproto::Request::TrashList): items from beneath
    /// `dir` after item `after`, oldest first, within one message.
    pub(crate) fn trash_list(&mut self, dir: u64, after: Option<u64>) -> Result<Reply, Error> {
        let mut rest = self
            .beneath(dir)?
            .into_iter()
            .filter(|(t, _)| after.is_none_or(|id| t.folder > id))
            .peekable();
        let (mut items, mut text) = (Vec::new(), 0);
        while let Some((_, path)) = rest.peek() {
            let full = items.len() == MAX_TRASH_ITEMS;
            if !items.is_empty() && (full || text + path.len() > MAX_TRASH_TEXT) {
                break;
            }
            let (t, path) = rest.next().expect("peeked");
            let path = tail(path);
            text += path.len();
            let inode = self.fs().stat(t.ino).map_err(error)?;
            items.push(TrashItem {
                id: t.folder,
                path,
                kind: kind(inode.kind),
                deleted: t.info.deleted,
                action: t.info.action,
            });
        }
        Ok(Reply::Trash {
            items,
            more: rest.peek().is_some(),
        })
    }

    /// [`Request::TrashRestore`](fsproto::Request::TrashRestore): the newest item
    /// that was at `path` beneath `dir` goes back there.
    pub(crate) fn trash_restore(&mut self, dir: u64, path: &str) -> Result<Reply, Error> {
        let (parent, name) = self.walk(dir, path)?;
        let name = name.ok_or(Error::Invalid)?;
        let item = self
            .beneath(dir)?
            .into_iter()
            .rev()
            .find(|(_, at)| at == path)
            .map(|(t, _)| t)
            .ok_or(Error::NotFound)?;
        let fs = self.fs();
        if fs.lookup(parent, name).map_err(error)?.is_some() {
            return Err(Error::Exists);
        }
        fs.rename(item.folder, &item.name, parent, name)
            .map_err(error)?;
        fs.remove(item.folder, INFO).map_err(error)?;
        self.drop_slot(item.folder, false)?;
        Ok(Reply::Done)
    }

    /// [`Request::EmptyTrash`](fsproto::Request::EmptyTrash): every item from
    /// beneath `dir` goes for good.
    pub(crate) fn empty_trash(&mut self, dir: u64) -> Result<Reply, Error> {
        let items = self.beneath(dir)?;
        for (item, _) in &items {
            self.drop_slot(item.folder, true)?;
        }
        Ok(Reply::Emptied {
            count: items.len() as u64,
        })
    }

    /// The trashed items that were beneath `dir`, oldest first, each with its
    /// path relative to `dir`.
    fn beneath(&mut self, dir: u64) -> Result<Vec<(Trashed, String)>, Error> {
        let fs = self.fs();
        let base = fs.path(None, dir).map_err(error)?.ok_or(Error::NotFound)?;
        let prefix = match base.as_str() {
            "/" => String::from("/"),
            _ => format!("{base}/"),
        };
        let mut items: Vec<(Trashed, String)> = fs
            .trash_list()
            .map_err(error)?
            .into_iter()
            .filter_map(|t| {
                let path = t.info.path.strip_prefix(prefix.as_str())?;
                let path = String::from(path);
                (!path.is_empty()).then_some((t, path))
            })
            .collect();
        // Object ids only grow, so a cursor by id holds while items go.
        items.sort_by_key(|(t, _)| t.folder);
        Ok(items)
    }

    /// Removes the trash folder `/.trash/<txn>/<n>` (`folder`): with what is in
    /// it when `all`, else only once emptied; then `<txn>` if nothing is left.
    fn drop_slot(&mut self, folder: u64, all: bool) -> Result<(), Error> {
        let fs = self.fs();
        let path = fs
            .path(None, folder)
            .map_err(error)?
            .ok_or(Error::NotFound)?;
        let mut parts = path.trim_start_matches('/').split('/');
        let (Some(TRASH), Some(txn), Some(n), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(Error::Io);
        };
        let trash = fs
            .lookup(ROOT, TRASH)
            .map_err(error)?
            .ok_or(Error::NotFound)?
            .ino;
        let slots = fs
            .lookup(trash, txn)
            .map_err(error)?
            .ok_or(Error::NotFound)?
            .ino;
        match all {
            true => fs.remove_tree(slots, n),
            false => fs.remove(slots, n),
        }
        .map_err(error)?;
        if fs.list(slots, None, 1).map_err(error)?.is_empty() {
            fs.remove(trash, txn).map_err(error)?;
        }
        Ok(())
    }
}

/// `path`, or its last [`MAX_TRASH_TEXT`] bytes when longer, cut at a
/// character.
fn tail(path: String) -> String {
    if path.len() <= MAX_TRASH_TEXT {
        return path;
    }
    let mut at = path.len() - MAX_TRASH_TEXT;
    while !path.is_char_boundary(at) {
        at += 1;
    }
    String::from(&path[at..])
}
