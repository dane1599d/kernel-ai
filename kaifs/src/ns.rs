//! The namespace's operations (ADR 0004 §2, §6; P2 task 5): names, folders,
//! whole-file and ranged writes, rename, remove and attributes, on the items
//! [`crate::items`] lays out.
//!
//! - **Inode numbers** come from the volume's counter ([`Fs::alloc_oid`]) and are
//!   never reused, even across a restore, so a handle to a removed file never
//!   names a new one. The root folder is inode 1 and is its own parent.
//! - **Data.** A hole is an absent item, or the bytes past an item's end within
//!   its 128 KiB; both read as zeros. A write inside a record rewrites the whole
//!   record, the cost ADR 0004 accepts.
//! - **Atomicity.** Every operation lands in the transaction's one commit, so a
//!   whole-file write, a rename across folders or a remove is wholly done or not
//!   at all after a crash. An operation checks everything it can refuse before it
//!   changes anything; an I/O error midway leaves the transaction to be dropped.

use crate::Error;
use crate::dev::BlockDev;
use crate::fs::{Fs, MAX_INLINE, SYSTEM, VOLUMES, decode_value, inline_of, value_len};
use crate::items::{
    CHUNK, Entry, FileType, Inode, K_DATA, K_DIRENT, K_INODE, K_XATTR, MAX_FILE, ROOT,
    SEALED_CHUNK, decode_attrs, decode_names, encode_attrs, encode_names, hash_name,
    valid_attr_name, valid_name,
};
use crate::tree::Key;
use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// A name in a folder listing, with its hash: pass it back to go on after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub hash: u64,
    pub entry: Entry,
}

/// A namespace item's bytes: namespace items other than DATA are always inline.
fn inline(raw: &[u8], what: &'static str) -> Result<Vec<u8>, Error> {
    inline_of(raw)
        .map(<[u8]>::to_vec)
        .ok_or(Error::Corrupt(what))
}

impl<D: BlockDev> Fs<D> {
    /// Writes an empty filesystem over all of `dev` whose volumes `system`, `home`
    /// and `models` each hold an empty root folder, as generation 1: `cargo xtask
    /// fs mkfs`.
    pub fn mkfs(dev: D, uuid: [u8; 16], label: &str) -> Result<Self, Error> {
        Self::mkfs_with(dev, uuid, label, crate::fs::Faults::default())
    }

    /// [`Fs::mkfs`] with deliberate faults, for the crash gates.
    pub fn mkfs_with(
        dev: D,
        uuid: [u8; 16],
        label: &str,
        faults: crate::fs::Faults,
    ) -> Result<Self, Error> {
        let mut fs = Self::build(dev, uuid, label, faults, &VOLUMES)?;
        for (vol, _) in VOLUMES {
            fs.set_volume(vol)?;
            let root = fs.alloc_oid()?;
            debug_assert_eq!(root, ROOT);
            let inode = Inode::new(FileType::Dir, ROOT, fs.clock);
            fs.put_inode(ROOT, &inode)?;
        }
        fs.set_volume(SYSTEM)?;
        fs.commit()?;
        Ok(fs)
    }

    pub fn name_hash(&self, name: &str) -> u64 {
        hash_name(&self.uuid, self.hash_mask, name)
    }

    pub(crate) fn writing(&self) -> Result<(), Error> {
        match self.writable() {
            true => Ok(()),
            false => Err(Error::ReadOnly),
        }
    }

    pub fn stat(&mut self, ino: u64) -> Result<Inode, Error> {
        let raw = self
            .get_raw(&Key::new(ino, K_INODE, 0))?
            .ok_or(Error::NotFound)?;
        Inode::decode(&inline(&raw, "inode")?)
    }

    /// Writes `ino`'s inode as changed in this transaction, and notes it for the
    /// open action's UNDO record.
    pub(crate) fn put_inode(&mut self, ino: u64, inode: &Inode) -> Result<(), Error> {
        let inode = Inode {
            changed: self.io.txn,
            ..*inode
        };
        self.touch(ino);
        self.put(Key::new(ino, K_INODE, 0), &inode.encode())
    }

    pub(crate) fn dir(&mut self, ino: u64) -> Result<Inode, Error> {
        let inode = self.stat(ino)?;
        match inode.kind {
            FileType::Dir => Ok(inode),
            FileType::File => Err(Error::NotDir),
        }
    }

    pub(crate) fn file(&mut self, ino: u64) -> Result<Inode, Error> {
        let inode = self.stat(ino)?;
        match inode.kind {
            FileType::File => Ok(inode),
            FileType::Dir => Err(Error::IsDir),
        }
    }

    /// The names in `dir` whose hash is `hash`.
    pub(crate) fn names(&mut self, dir: u64, hash: u64) -> Result<Vec<Entry>, Error> {
        match self.get_raw(&Key::new(dir, K_DIRENT, hash))? {
            None => Ok(Vec::new()),
            Some(raw) => decode_names(&inline(&raw, "folder names")?),
        }
    }

    /// The DIRENT value for `entries`, if one item holds them.
    pub(crate) fn names_value(entries: &[Entry]) -> Result<Vec<u8>, Error> {
        let value = encode_names(entries);
        match value.len() <= MAX_INLINE {
            true => Ok(value),
            false => Err(Error::Invalid("too many names share a hash in this folder")),
        }
    }

    pub(crate) fn put_names(
        &mut self,
        dir: u64,
        hash: u64,
        value: Option<Vec<u8>>,
    ) -> Result<(), Error> {
        let key = Key::new(dir, K_DIRENT, hash);
        match value {
            Some(value) => self.put(key, &value),
            None => self.delete(&key).map(|_| ()),
        }
    }

    pub fn lookup(&mut self, dir: u64, name: &str) -> Result<Option<Entry>, Error> {
        self.dir(dir)?;
        if !valid_name(name) {
            return Ok(None);
        }
        let hash = self.name_hash(name);
        Ok(self.names(dir, hash)?.into_iter().find(|e| e.name == name))
    }

    /// The inode an absolute path names: `/` is the root, and `.`, `..` and empty
    /// components are refused (lookup beneath a folder is the `fs` server's).
    pub fn resolve(&mut self, path: &str) -> Result<u64, Error> {
        let rest = path
            .strip_prefix('/')
            .ok_or(Error::Invalid("a path not from /"))?;
        let mut ino = ROOT;
        if rest.is_empty() {
            return Ok(ino);
        }
        for part in rest.split('/') {
            if !valid_name(part) {
                return Err(Error::Invalid("a path component"));
            }
            ino = self.lookup(ino, part)?.ok_or(Error::NotFound)?.ino;
        }
        Ok(ino)
    }

    /// Up to `max` names in `dir`, in hash order, after `after` if given.
    pub fn list(
        &mut self,
        dir: u64,
        after: Option<&Listed>,
        max: usize,
    ) -> Result<Vec<Listed>, Error> {
        self.dir(dir)?;
        let from = Key::new(dir, K_DIRENT, after.map_or(0, |l| l.hash));
        let mut out: Vec<Listed> = Vec::new();
        let mut failed = None;
        self.scan_raw(&from, &mut |key, raw| {
            if key.oid != dir || key.kind != K_DIRENT {
                return false;
            }
            let names = inline(raw, "folder names").and_then(|v| decode_names(&v));
            match names {
                Err(e) => failed = Some(e),
                Ok(names) => {
                    let skip = |e: &Entry| {
                        after.is_some_and(|l| l.hash == key.off && e.name <= l.entry.name)
                    };
                    for entry in names.into_iter().filter(|e| !skip(e)) {
                        out.push(Listed {
                            hash: key.off,
                            entry,
                        });
                    }
                }
            }
            failed.is_none() && out.len() < max
        })?;
        if let Some(e) = failed {
            return Err(e);
        }
        out.truncate(max);
        Ok(out)
    }

    /// Makes an empty file named `name` in `dir`; refuses a name that is taken.
    pub fn create(&mut self, dir: u64, name: &str) -> Result<u64, Error> {
        self.make(dir, name, FileType::File)
    }

    /// Makes an empty folder named `name` in `dir`.
    pub fn mkdir(&mut self, dir: u64, name: &str) -> Result<u64, Error> {
        self.make(dir, name, FileType::Dir)
    }

    fn make(&mut self, dir: u64, name: &str, kind: FileType) -> Result<u64, Error> {
        self.writing()?;
        if !valid_name(name) {
            return Err(Error::Invalid("a name"));
        }
        let mut parent = self.dir(dir)?;
        let hash = self.name_hash(name);
        let mut entries = self.names(dir, hash)?;
        let at = match entries.binary_search_by(|e| e.name.as_str().cmp(name)) {
            Ok(_) => return Err(Error::Exists),
            Err(at) => at,
        };
        let subdirs = u32::from(kind == FileType::Dir);
        parent.nlink = parent
            .nlink
            .checked_add(subdirs)
            .ok_or(Error::Invalid("too many folders in one folder"))?;
        let ino = self.next_oid();
        entries.insert(
            at,
            Entry {
                name: name.to_string(),
                ino,
                kind,
            },
        );
        let value = Self::names_value(&entries)?;
        assert_eq!(self.alloc_oid()?, ino);
        let now = self.clock;
        self.put_inode(ino, &Inode::new(kind, dir, now))?;
        self.put_names(dir, hash, Some(value))?;
        parent.size = parent.size.saturating_add(1);
        (parent.mtime, parent.ctime) = (now, now);
        self.put_inode(dir, &parent)?;
        Ok(ino)
    }

    /// Names `ino`, of type `kind`, `name` in the folder `dir`: `restore` puts an
    /// object back this way.
    pub(crate) fn link(
        &mut self,
        dir: u64,
        name: &str,
        ino: u64,
        kind: FileType,
    ) -> Result<(), Error> {
        if !valid_name(name) {
            return Err(Error::Invalid("a name"));
        }
        let mut parent = self.dir(dir)?;
        let hash = self.name_hash(name);
        let mut entries = self.names(dir, hash)?;
        let at = match entries.binary_search_by(|e| e.name.as_str().cmp(name)) {
            Ok(_) => return Err(Error::Exists),
            Err(at) => at,
        };
        entries.insert(
            at,
            Entry {
                name: name.to_string(),
                ino,
                kind,
            },
        );
        let value = Self::names_value(&entries)?;
        parent.nlink = parent
            .nlink
            .checked_add(u32::from(kind == FileType::Dir))
            .ok_or(Error::Invalid("too many folders in one folder"))?;
        self.put_names(dir, hash, Some(value))?;
        parent.size = parent.size.saturating_add(1);
        (parent.mtime, parent.ctime) = (self.clock, self.clock);
        self.put_inode(dir, &parent)
    }

    /// Removes the name `name` from `dir`, and the file or empty folder it named.
    pub fn remove(&mut self, dir: u64, name: &str) -> Result<(), Error> {
        self.writing()?;
        let mut parent = self.dir(dir)?;
        let hash = self.name_hash(name);
        let mut entries = self.names(dir, hash)?;
        let at = entries
            .iter()
            .position(|e| e.name == name)
            .ok_or(Error::NotFound)?;
        let entry = entries.remove(at);
        let mut child = self.stat(entry.ino)?;
        if child.kind != entry.kind {
            return Err(Error::Corrupt("a name's type"));
        }
        if child.kind == FileType::Dir && child.size > 0 {
            return Err(Error::NotEmpty);
        }
        let value = match entries.is_empty() {
            true => None,
            false => Some(Self::names_value(&entries)?),
        };
        self.put_names(dir, hash, value)?;
        let now = self.clock;
        parent.size = parent.size.saturating_sub(1);
        if child.kind == FileType::Dir {
            parent.nlink = parent.nlink.saturating_sub(1);
        }
        (parent.mtime, parent.ctime) = (now, now);
        self.put_inode(dir, &parent)?;
        child.nlink = child.nlink.saturating_sub(1);
        match child.kind == FileType::Dir || child.nlink == 0 {
            true => self.destroy(entry.ino),
            false => {
                child.ctime = now;
                self.put_inode(entry.ino, &child)
            }
        }
    }

    /// Removes every item of `ino`; its records are freed by the reuse rule.
    pub(crate) fn destroy(&mut self, ino: u64) -> Result<(), Error> {
        self.touch(ino);
        let mut keys: Vec<Key> = Vec::new();
        self.scan_raw(&Key::new(ino, 0, 0), &mut |key, _| {
            let mine = key.oid == ino;
            if mine {
                keys.push(*key);
            }
            mine
        })?;
        for key in keys {
            self.delete(&key)?;
        }
        Ok(())
    }

    /// Moves the name `from_name` in `from` to `to_name` in `to`. A name that is
    /// taken is refused: replacing is a remove and a rename in one transaction. A
    /// folder cannot move into itself or below itself.
    pub fn rename(
        &mut self,
        from: u64,
        from_name: &str,
        to: u64,
        to_name: &str,
    ) -> Result<(), Error> {
        self.writing()?;
        if !valid_name(to_name) {
            return Err(Error::Invalid("a name"));
        }
        let mut src_dir = self.dir(from)?;
        let mut dst_dir = self.dir(to)?;
        let src_hash = self.name_hash(from_name);
        let mut src = self.names(from, src_hash)?;
        let at = src
            .iter()
            .position(|e| e.name == from_name)
            .ok_or(Error::NotFound)?;
        if from == to && from_name == to_name {
            return Ok(());
        }
        let mut entry = src.remove(at);
        let mut child = self.stat(entry.ino)?;
        if child.kind != entry.kind {
            return Err(Error::Corrupt("a name's type"));
        }
        if child.kind == FileType::Dir && from != to {
            self.refuse_cycle(entry.ino, to)?;
        }
        let dst_hash = self.name_hash(to_name);
        let same_item = from == to && src_hash == dst_hash;
        let mut dst = match same_item {
            true => core::mem::take(&mut src),
            false => self.names(to, dst_hash)?,
        };
        let at = match dst.binary_search_by(|e| e.name.as_str().cmp(to_name)) {
            Ok(_) => return Err(Error::Exists),
            Err(at) => at,
        };
        entry.name = to_name.to_string();
        let subdir = u32::from(child.kind == FileType::Dir);
        if from != to {
            dst_dir.nlink = dst_dir
                .nlink
                .checked_add(subdir)
                .ok_or(Error::Invalid("too many folders in one folder"))?;
        }
        dst.insert(at, entry.clone());
        let dst_value = Self::names_value(&dst)?;
        let src_value = match (same_item, src.is_empty()) {
            (true, _) | (false, true) => None,
            (false, false) => Some(Self::names_value(&src)?),
        };
        if !same_item {
            self.put_names(from, src_hash, src_value)?;
        }
        self.put_names(to, dst_hash, Some(dst_value))?;
        let now = self.clock;
        if from != to {
            src_dir.size = src_dir.size.saturating_sub(1);
            src_dir.nlink = src_dir.nlink.saturating_sub(subdir);
            (src_dir.mtime, src_dir.ctime) = (now, now);
            self.put_inode(from, &src_dir)?;
            dst_dir.size = dst_dir.size.saturating_add(1);
            child.parent = to;
        }
        (dst_dir.mtime, dst_dir.ctime) = (now, now);
        self.put_inode(to, &dst_dir)?;
        child.ctime = now;
        self.put_inode(entry.ino, &child)
    }

    /// Refuses to move the folder `moved` into `to` if `to` is `moved` or lies
    /// below it: walks `to`'s parents up to the root.
    fn refuse_cycle(&mut self, moved: u64, to: u64) -> Result<(), Error> {
        let mut seen = BTreeSet::new();
        let mut ino = to;
        loop {
            if ino == moved {
                return Err(Error::Invalid("a folder cannot move below itself"));
            }
            if ino == ROOT {
                return Ok(());
            }
            if !seen.insert(ino) {
                return Err(Error::Corrupt("a folder's parents loop"));
            }
            ino = self.dir(ino)?.parent;
        }
    }

    /// The raw DATA items of `ino` from offset `from` on: (offset, raw value).
    pub(crate) fn chunks(
        &mut self,
        ino: u64,
        from: u64,
        to: u64,
    ) -> Result<Vec<(u64, Vec<u8>)>, Error> {
        let mut out = Vec::new();
        self.scan_raw(&Key::new(ino, K_DATA, from), &mut |key, raw| {
            let mine = key.oid == ino && key.kind == K_DATA && key.off < to;
            if mine {
                out.push((key.off, raw.to_vec()));
            }
            mine
        })?;
        Ok(out)
    }

    /// Up to `len` bytes of `ino` from `off`, holes as zeros; fewer past the end.
    pub fn read_at(&mut self, ino: u64, off: u64, len: usize) -> Result<Vec<u8>, Error> {
        let inode = self.file(ino)?;
        let end = off.saturating_add(len as u64).min(inode.size);
        if off >= end {
            return Ok(Vec::new());
        }
        let mut out = vec![0; (end - off) as usize];
        // A sealed file's item holding `off` may start up to 2 MiB before it;
        // the items that end before `off` are passed over unread.
        for (at, raw) in self.chunks(ino, off / SEALED_CHUNK * SEALED_CHUNK, end)? {
            let len = value_len(&raw).ok_or(Error::Corrupt("a data item"))?;
            if at.saturating_add(len as u64) <= off {
                continue;
            }
            let value = decode_value(&raw, &mut self.io)?;
            let bytes = value.bytes();
            let lo = off.max(at);
            let hi = end.min(at.saturating_add(bytes.len() as u64));
            if lo < hi {
                out[(lo - off) as usize..(hi - off) as usize]
                    .copy_from_slice(&bytes[(lo - at) as usize..(hi - at) as usize]);
            }
        }
        Ok(out)
    }

    pub fn read_file(&mut self, ino: u64) -> Result<Vec<u8>, Error> {
        let size = self.file(ino)?.size;
        let len =
            usize::try_from(size).map_err(|_| Error::Invalid("a file this host cannot hold"))?;
        self.read_at(ino, 0, len)
    }

    /// Sets a data item to `bytes`: none if empty, inline if they fit, else a
    /// record.
    fn put_chunk(&mut self, ino: u64, at: u64, bytes: &[u8]) -> Result<(), Error> {
        let key = Key::new(ino, K_DATA, at);
        match bytes.len() {
            0 => self.delete(&key).map(|_| ()),
            n if n <= MAX_INLINE => self.put(key, bytes),
            _ => self.put_record(key, bytes),
        }
    }

    /// Writes `data` into `ino` at `off`, growing it if they end past it.
    pub fn write_at(&mut self, ino: u64, off: u64, data: &[u8]) -> Result<(), Error> {
        self.writing()?;
        let mut inode = self.file(ino)?;
        self.unsealed(ino)?;
        let end = off
            .checked_add(data.len() as u64)
            .filter(|&end| end <= MAX_FILE)
            .ok_or(Error::Invalid("a file over 256 TiB"))?;
        let mut at = off;
        while at < end {
            let chunk = at / CHUNK * CHUNK;
            let stop = end.min(chunk + CHUNK);
            let (lo, hi) = ((at - chunk) as usize, (stop - chunk) as usize);
            let key = Key::new(ino, K_DATA, chunk);
            let mut bytes = match self.get_raw(&key)? {
                None => Vec::new(),
                // Every byte it holds is overwritten: no need to read it.
                Some(raw) if lo == 0 && value_len(&raw).is_some_and(|len| len <= hi) => Vec::new(),
                Some(raw) => decode_value(&raw, &mut self.io)?.into_bytes(),
            };
            if bytes.len() < hi {
                bytes.resize(hi, 0);
            }
            bytes[lo..hi].copy_from_slice(&data[(at - off) as usize..(stop - off) as usize]);
            self.put_chunk(ino, chunk, &bytes)?;
            at = stop;
        }
        inode.size = inode.size.max(end);
        (inode.mtime, inode.ctime) = (self.clock, self.clock);
        self.put_inode(ino, &inode)
    }

    /// Sets `ino`'s size: bytes past it are dropped, and a larger size reads as
    /// zeros.
    pub fn truncate(&mut self, ino: u64, size: u64) -> Result<(), Error> {
        self.writing()?;
        let mut inode = self.file(ino)?;
        self.unsealed(ino)?;
        if size > MAX_FILE {
            return Err(Error::Invalid("a file over 256 TiB"));
        }
        if size < inode.size {
            for (at, raw) in self.chunks(ino, size / CHUNK * CHUNK, u64::MAX)? {
                let len = value_len(&raw).ok_or(Error::Corrupt("a data item"))? as u64;
                if at >= size {
                    self.delete(&Key::new(ino, K_DATA, at))?;
                } else if at.saturating_add(len) > size {
                    let mut bytes = decode_value(&raw, &mut self.io)?.into_bytes();
                    bytes.truncate((size - at) as usize);
                    self.put_chunk(ino, at, &bytes)?;
                }
            }
        }
        inode.size = size;
        (inode.mtime, inode.ctime) = (self.clock, self.clock);
        self.put_inode(ino, &inode)
    }

    /// Replaces all of `ino`'s bytes with `data`. With the rest of the
    /// transaction it is one commit, so a crash leaves the old bytes or the new.
    pub fn write_file(&mut self, ino: u64, data: &[u8]) -> Result<(), Error> {
        self.truncate(ino, 0)?;
        self.write_at(ino, 0, data)
    }

    fn attrs_at(&mut self, ino: u64, hash: u64) -> Result<Vec<(String, Vec<u8>)>, Error> {
        match self.get_raw(&Key::new(ino, K_XATTR, hash))? {
            None => Ok(Vec::new()),
            Some(raw) => decode_attrs(&inline(&raw, "attributes")?),
        }
    }

    /// Sets the extended attribute `name` of `ino`; all of a name hash's
    /// attributes share one item, so values are small.
    pub fn set_xattr(&mut self, ino: u64, name: &str, value: &[u8]) -> Result<(), Error> {
        self.writing()?;
        if !valid_attr_name(name) {
            return Err(Error::Invalid("an attribute name"));
        }
        let mut inode = self.stat(ino)?;
        let hash = self.name_hash(name);
        let mut attrs = self.attrs_at(ino, hash)?;
        match attrs.binary_search_by(|(n, _)| n.as_str().cmp(name)) {
            Ok(i) => attrs[i].1 = value.to_vec(),
            Err(i) => attrs.insert(i, (name.to_string(), value.to_vec())),
        }
        let raw = encode_attrs(&attrs);
        if raw.len() > MAX_INLINE {
            return Err(Error::Invalid("attributes over what one item holds"));
        }
        self.put(Key::new(ino, K_XATTR, hash), &raw)?;
        inode.ctime = self.clock;
        self.put_inode(ino, &inode)
    }

    pub fn get_xattr(&mut self, ino: u64, name: &str) -> Result<Option<Vec<u8>>, Error> {
        self.stat(ino)?;
        if !valid_attr_name(name) {
            return Ok(None);
        }
        let hash = self.name_hash(name);
        let attrs = self.attrs_at(ino, hash)?;
        Ok(attrs.into_iter().find(|(n, _)| n == name).map(|(_, v)| v))
    }

    /// Removes the attribute `name`; returns whether it was there.
    pub fn remove_xattr(&mut self, ino: u64, name: &str) -> Result<bool, Error> {
        self.writing()?;
        let mut inode = self.stat(ino)?;
        if !valid_attr_name(name) {
            return Ok(false);
        }
        let hash = self.name_hash(name);
        let mut attrs = self.attrs_at(ino, hash)?;
        let Some(at) = attrs.iter().position(|(n, _)| n == name) else {
            return Ok(false);
        };
        attrs.remove(at);
        let key = Key::new(ino, K_XATTR, hash);
        match attrs.is_empty() {
            true => self.delete(&key).map(|_| ())?,
            false => self.put(key, &encode_attrs(&attrs))?,
        }
        inode.ctime = self.clock;
        self.put_inode(ino, &inode)?;
        Ok(true)
    }

    /// Every extended attribute of `ino`, in hash order.
    pub fn xattrs(&mut self, ino: u64) -> Result<Vec<(String, Vec<u8>)>, Error> {
        self.stat(ino)?;
        let mut raws: Vec<Vec<u8>> = Vec::new();
        self.scan_raw(&Key::new(ino, K_XATTR, 0), &mut |key, raw| {
            let mine = key.oid == ino && key.kind == K_XATTR;
            if mine {
                raws.push(raw.to_vec());
            }
            mine
        })?;
        let mut out = Vec::new();
        for raw in raws {
            out.extend(decode_attrs(&inline(&raw, "attributes")?)?);
        }
        Ok(out)
    }
}
