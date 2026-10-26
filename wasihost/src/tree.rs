//! The in-memory file tree: `/kai`, read-only to the guest (headers, `kai.h`,
//! `kairt`), and `/w`, the compile's scratch, with `/tmp` as more scratch: wcc
//! preprocesses into a `tmpfile()` there. Paths are absolute and normalised;
//! a guest path is joined to its directory only through [`join`], which refuses
//! `..`, so no path escapes its preopen.

use crate::errno::Errno;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

/// The read-only root.
pub const KAI: &str = "/kai";
/// The scratch root.
pub const SCRATCH: &str = "/w";
/// Scratch for libc's `tmpfile()` and `mkstemp()`.
pub const TMP: &str = "/tmp";

/// Limits on the scratch tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeLimits {
    /// Most bytes in all scratch files (`/w` and `/tmp`) together.
    pub scratch_bytes: usize,
    /// Most files and directories in the scratch trees.
    pub scratch_entries: usize,
}

impl Default for TreeLimits {
    fn default() -> Self {
        Self {
            scratch_bytes: 32 << 20,
            scratch_entries: 256,
        }
    }
}

/// What a path names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
}

/// A file's bytes, whether a path names it, and how many fds hold it open.
#[derive(Clone, Debug)]
struct Inode {
    data: Vec<u8>,
    linked: bool,
    opens: u32,
    writable: bool,
}

/// Files and directories by absolute path. A file is an inode, so one unlinked
/// while open lives on until its last fd closes, as libc's `tmpfile()` expects.
#[derive(Clone, Debug)]
pub struct Tree {
    files: BTreeMap<String, u32>,
    inodes: BTreeMap<u32, Inode>,
    next_inode: u32,
    dirs: BTreeSet<String>,
    limits: TreeLimits,
    scratch_used: usize,
}

impl Default for Tree {
    fn default() -> Self {
        Self::new(TreeLimits::default())
    }
}

impl Tree {
    pub fn new(limits: TreeLimits) -> Self {
        let mut dirs = BTreeSet::new();
        dirs.insert(String::from(KAI));
        dirs.insert(String::from(SCRATCH));
        dirs.insert(String::from(TMP));
        Self {
            files: BTreeMap::new(),
            inodes: BTreeMap::new(),
            next_inode: 1,
            dirs,
            limits,
            scratch_used: 0,
        }
    }

    /// Adds a read-only file under `/kai` from the host, with its parent
    /// directories. `path` is relative to `/kai`.
    pub fn add_kai(&mut self, path: &str, data: Vec<u8>) -> Result<(), Errno> {
        let full = join(KAI, path.as_bytes())?;
        let mut parent = parent(&full);
        while parent.len() > KAI.len() {
            self.dirs.insert(String::from(parent));
            parent = self::parent(parent);
        }
        if let Some(old) = self.files.remove(&full) {
            self.inodes.remove(&old);
        }
        let inode = self.new_inode(data, false);
        self.files.insert(full, inode);
        Ok(())
    }

    /// Writes a file in `/w` from the host, replacing any file there; it counts
    /// against the limits. `path` is relative to `/w`, and its directory must exist.
    pub fn put_scratch(&mut self, path: &str, data: &[u8]) -> Result<(), Errno> {
        let full = join(SCRATCH, path.as_bytes())?;
        let inode = match self.files.get(&full) {
            Some(&inode) => {
                self.truncate(inode)?;
                inode
            }
            None => self.create(&full)?,
        };
        self.write_at(inode, 0, data)
    }

    /// Reads a file by absolute path, for the host.
    pub fn read(&self, path: &str) -> Option<&[u8]> {
        let inode = self.files.get(path)?;
        self.inodes.get(inode).map(|i| i.data.as_slice())
    }

    /// Empties `/w` and `/tmp`, for the next compile. No fd may still use them:
    /// call it on a tree taken back with [`crate::Wasi::into_tree`].
    pub fn clear_scratch(&mut self) {
        self.files.retain(|path, _| !writable(path));
        self.inodes.retain(|_, inode| !inode.writable);
        self.dirs.retain(|path| !writable(path) || is_root(path));
        self.scratch_used = 0;
    }

    /// Bytes in the scratch files, unlinked ones still open included.
    pub fn scratch_used(&self) -> usize {
        self.scratch_used
    }

    pub fn kind(&self, path: &str) -> Option<Kind> {
        if self.files.contains_key(path) {
            Some(Kind::File)
        } else if self.dirs.contains(path) {
            Some(Kind::Dir)
        } else {
            None
        }
    }

    /// The inode a file path names.
    pub(crate) fn inode(&self, path: &str) -> Option<u32> {
        self.files.get(path).copied()
    }

    pub(crate) fn data(&self, inode: u32) -> Result<&[u8], Errno> {
        self.inodes
            .get(&inode)
            .map(|i| i.data.as_slice())
            .ok_or(Errno::BADF)
    }

    fn new_inode(&mut self, data: Vec<u8>, writable: bool) -> u32 {
        let id = self.next_inode;
        self.next_inode += 1;
        let inode = Inode {
            data,
            linked: true,
            opens: 0,
            writable,
        };
        self.inodes.insert(id, inode);
        id
    }

    /// An fd now holds `inode` open.
    pub(crate) fn open(&mut self, inode: u32) {
        if let Some(i) = self.inodes.get_mut(&inode) {
            i.opens += 1;
        }
    }

    /// An fd holding `inode` closed; an unlinked inode goes with its last fd.
    pub(crate) fn close(&mut self, inode: u32) {
        if let Some(i) = self.inodes.get_mut(&inode) {
            i.opens = i.opens.saturating_sub(1);
            self.drop_if_unused(inode);
        }
    }

    fn drop_if_unused(&mut self, inode: u32) {
        let unused = self
            .inodes
            .get(&inode)
            .is_some_and(|i| !i.linked && i.opens == 0);
        if unused
            && let Some(i) = self.inodes.remove(&inode)
            && i.writable
        {
            self.scratch_used -= i.data.len();
        }
    }

    fn entries(&self) -> usize {
        let files = self.inodes.values().filter(|i| i.writable).count();
        let dirs = self.dirs.iter().filter(|p| writable(p) && !is_root(p));
        files + dirs.count()
    }

    fn room_for_entry(&self) -> Result<(), Errno> {
        if self.entries() >= self.limits.scratch_entries {
            return Err(Errno::NOSPC);
        }
        Ok(())
    }

    /// Creates an empty file; its parent must be a scratch directory.
    pub(crate) fn create(&mut self, path: &str) -> Result<u32, Errno> {
        self.check_new(path)?;
        let inode = self.new_inode(Vec::new(), true);
        self.files.insert(String::from(path), inode);
        Ok(inode)
    }

    pub(crate) fn mkdir(&mut self, path: &str) -> Result<(), Errno> {
        self.check_new(path)?;
        self.dirs.insert(String::from(path));
        Ok(())
    }

    fn check_new(&self, path: &str) -> Result<(), Errno> {
        if !writable(path) {
            return Err(Errno::ROFS);
        }
        if self.kind(path).is_some() {
            return Err(Errno::EXIST);
        }
        match self.kind(parent(path)) {
            Some(Kind::Dir) => {}
            Some(Kind::File) => return Err(Errno::NOTDIR),
            None => return Err(Errno::NOENT),
        }
        self.room_for_entry()
    }

    /// Writes `data` at `pos` in a scratch file, zero-filling any gap.
    pub(crate) fn write_at(&mut self, inode: u32, pos: u64, data: &[u8]) -> Result<(), Errno> {
        let file = self.inodes.get(&inode).ok_or(Errno::BADF)?;
        if !file.writable {
            return Err(Errno::ROFS);
        }
        let pos = usize::try_from(pos).map_err(|_| Errno::FBIG)?;
        let end = pos.checked_add(data.len()).ok_or(Errno::FBIG)?;
        let growth = end.saturating_sub(file.data.len());
        let used = self.scratch_used.checked_add(growth).ok_or(Errno::NOSPC)?;
        if used > self.limits.scratch_bytes {
            return Err(Errno::NOSPC);
        }
        let file = &mut self.inodes.get_mut(&inode).ok_or(Errno::BADF)?.data;
        if file.len() < end {
            file.resize(end, 0);
        }
        file[pos..end].copy_from_slice(data);
        self.scratch_used = used;
        Ok(())
    }

    pub(crate) fn truncate(&mut self, inode: u32) -> Result<(), Errno> {
        let file = self.inodes.get_mut(&inode).ok_or(Errno::BADF)?;
        if !file.writable {
            return Err(Errno::ROFS);
        }
        self.scratch_used -= file.data.len();
        file.data.clear();
        Ok(())
    }

    pub(crate) fn unlink(&mut self, path: &str) -> Result<(), Errno> {
        if !writable(path) {
            return Err(Errno::ROFS);
        }
        match self.kind(path) {
            Some(Kind::File) => {}
            Some(Kind::Dir) => return Err(Errno::ISDIR),
            None => return Err(Errno::NOENT),
        }
        if let Some(inode) = self.files.remove(path) {
            if let Some(i) = self.inodes.get_mut(&inode) {
                i.linked = false;
            }
            self.drop_if_unused(inode);
        }
        Ok(())
    }

    pub(crate) fn rmdir(&mut self, path: &str) -> Result<(), Errno> {
        if !writable(path) {
            return Err(Errno::ROFS);
        }
        if is_root(path) {
            return Err(Errno::NOTCAPABLE);
        }
        match self.kind(path) {
            Some(Kind::Dir) => {}
            Some(Kind::File) => return Err(Errno::NOTDIR),
            None => return Err(Errno::NOENT),
        }
        let prefix = alloc::format!("{path}/");
        let mut busy = self.files.keys().chain(self.dirs.iter());
        if busy.any(|p| p.starts_with(&prefix)) {
            return Err(Errno::NOTEMPTY);
        }
        self.dirs.remove(path);
        Ok(())
    }

    /// Renames a file between scratch paths, replacing a file at `to`.
    /// Directories are not renamed.
    pub(crate) fn rename(&mut self, from: &str, to: &str) -> Result<(), Errno> {
        if !writable(from) || !writable(to) {
            return Err(Errno::ROFS);
        }
        match self.kind(from) {
            Some(Kind::File) => {}
            Some(Kind::Dir) => return Err(Errno::NOTSUP),
            None => return Err(Errno::NOENT),
        }
        if from == to {
            return Ok(());
        }
        match self.kind(to) {
            Some(Kind::Dir) => return Err(Errno::ISDIR),
            Some(Kind::File) => self.unlink(to)?,
            None if self.kind(parent(to)) != Some(Kind::Dir) => return Err(Errno::NOENT),
            None => {}
        }
        let inode = self.files.remove(from).ok_or(Errno::NOENT)?;
        self.files.insert(String::from(to), inode);
        Ok(())
    }
}

/// Whether `path` is in a scratch tree.
pub fn writable(path: &str) -> bool {
    [SCRATCH, TMP].iter().any(|root| {
        path.strip_prefix(root)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

fn is_root(path: &str) -> bool {
    path == SCRATCH || path == TMP
}

/// The directory `path` is in; a root is its own parent.
pub fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => path,
        Some(i) => &path[..i],
    }
}

/// `dir` joined with a guest path relative to it. Empty and `.` components are
/// dropped; an absolute path, `..`, a NUL, bad UTF-8 or a path longer than
/// [`crate::MAX_PATH`] is refused.
pub fn join(dir: &str, path: &[u8]) -> Result<String, Errno> {
    if path.len() > crate::MAX_PATH {
        return Err(Errno::NAMETOOLONG);
    }
    let path = core::str::from_utf8(path).map_err(|_| Errno::INVAL)?;
    if path.starts_with('/') || path.contains('\0') {
        return Err(Errno::NOTCAPABLE);
    }
    let mut out = String::from(dir);
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(Errno::NOTCAPABLE),
            name => {
                out.push('/');
                out.push_str(name);
            }
        }
    }
    Ok(out)
}
