//! The in-memory file tree: `/kai`, read-only to the guest (headers, `kai.h`,
//! `kairt`), and `/w`, the compile's scratch. Paths are absolute and normalised;
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

/// Limits on the scratch tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeLimits {
    /// Most bytes in all of `/w`'s files together.
    pub scratch_bytes: usize,
    /// Most files and directories in `/w`.
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

/// Files and directories by absolute path.
#[derive(Clone, Debug)]
pub struct Tree {
    files: BTreeMap<String, Vec<u8>>,
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
        Self {
            files: BTreeMap::new(),
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
        self.files.insert(full, data);
        Ok(())
    }

    /// Reads a file by absolute path, for the host.
    pub fn read(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    /// Empties `/w`, for the next compile.
    pub fn clear_scratch(&mut self) {
        self.files.retain(|path, _| !writable(path));
        self.dirs.retain(|path| !writable(path) || path == SCRATCH);
        self.scratch_used = 0;
    }

    /// Bytes in `/w`'s files.
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

    pub(crate) fn file(&self, path: &str) -> Result<&[u8], Errno> {
        self.files.get(path).map(Vec::as_slice).ok_or(Errno::BADF)
    }

    fn entries(&self) -> usize {
        let files = self.files.keys().filter(|p| writable(p)).count();
        files + self.dirs.iter().filter(|p| writable(p)).count() - 1
    }

    fn room_for_entry(&self) -> Result<(), Errno> {
        if self.entries() >= self.limits.scratch_entries {
            return Err(Errno::NOSPC);
        }
        Ok(())
    }

    /// Creates an empty file; its parent must be a directory in `/w`.
    pub(crate) fn create(&mut self, path: &str) -> Result<(), Errno> {
        self.check_new(path)?;
        self.files.insert(String::from(path), Vec::new());
        Ok(())
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

    /// Writes `data` at `pos` in a file in `/w`, zero-filling any gap.
    pub(crate) fn write_at(&mut self, path: &str, pos: u64, data: &[u8]) -> Result<(), Errno> {
        if !writable(path) {
            return Err(Errno::ROFS);
        }
        let file = self.files.get(path).ok_or(Errno::BADF)?;
        let pos = usize::try_from(pos).map_err(|_| Errno::FBIG)?;
        let end = pos.checked_add(data.len()).ok_or(Errno::FBIG)?;
        let growth = end.saturating_sub(file.len());
        let used = self.scratch_used.checked_add(growth).ok_or(Errno::NOSPC)?;
        if used > self.limits.scratch_bytes {
            return Err(Errno::NOSPC);
        }
        let file = self.files.get_mut(path).ok_or(Errno::BADF)?;
        if file.len() < end {
            file.resize(end, 0);
        }
        file[pos..end].copy_from_slice(data);
        self.scratch_used = used;
        Ok(())
    }

    pub(crate) fn truncate(&mut self, path: &str) -> Result<(), Errno> {
        if !writable(path) {
            return Err(Errno::ROFS);
        }
        let file = self.files.get_mut(path).ok_or(Errno::NOENT)?;
        self.scratch_used -= file.len();
        file.clear();
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
        if let Some(file) = self.files.remove(path) {
            self.scratch_used -= file.len();
        }
        Ok(())
    }

    pub(crate) fn rmdir(&mut self, path: &str) -> Result<(), Errno> {
        if !writable(path) {
            return Err(Errno::ROFS);
        }
        if path == SCRATCH {
            return Err(Errno::NOTCAPABLE);
        }
        match self.kind(path) {
            Some(Kind::Dir) => {}
            Some(Kind::File) => return Err(Errno::NOTDIR),
            None => return Err(Errno::NOENT),
        }
        let prefix = alloc::format!("{path}/");
        let busy = self.files.keys().chain(self.dirs.iter());
        if busy.into_iter().any(|p| p.starts_with(&prefix)) {
            return Err(Errno::NOTEMPTY);
        }
        self.dirs.remove(path);
        Ok(())
    }

    /// Renames a file within `/w`, replacing a file at `to`. Directories are not
    /// renamed.
    pub(crate) fn rename(&mut self, from: &str, to: &str) -> Result<(), Errno> {
        if !writable(from) || !writable(to) {
            return Err(Errno::ROFS);
        }
        match self.kind(from) {
            Some(Kind::File) => {}
            Some(Kind::Dir) => return Err(Errno::NOTSUP),
            None => return Err(Errno::NOENT),
        }
        match self.kind(to) {
            Some(Kind::Dir) => return Err(Errno::ISDIR),
            Some(Kind::File) => self.unlink(to)?,
            None if self.kind(parent(to)) != Some(Kind::Dir) => return Err(Errno::NOENT),
            None => {}
        }
        let data = self.files.remove(from).ok_or(Errno::NOENT)?;
        self.files.insert(String::from(to), data);
        Ok(())
    }
}

/// Whether `path` is in the scratch tree.
pub fn writable(path: &str) -> bool {
    path == SCRATCH || path.starts_with("/w/")
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
