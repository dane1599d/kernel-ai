//! The store's [`Volume`] over `kai-fs 1` (P4 task 1; ADR 0011 §12): `pkg`
//! holds a `Dir` handle for `/system/pkg`, and the store's paths, `pkg/...`, are
//! paths beneath it. Each request goes through a [`Transport`]: CALL_H in `pkg`,
//! fsserve's server itself in the host tests.
//!
//! - [`Volume::begin`] and [`Volume::commit`] are `fs`'s action, Begin then
//!   Commit: one kaifs commit with its undo record. Outside an action `fs` makes
//!   each change durable as it answers, and commit is a Sync.
//! - `fs` renames within one folder: `kai-fs 1` does not yet take the
//!   destination `Dir` with the request. So a download the store keeps in
//!   `pkg/store/.partial/<hex>` lies at `store/.partial.<hex>`, beside the
//!   objects, and its rename into the store is one folder's. Listings of
//!   `pkg/store` leave those names out and listings of `pkg/store/.partial` give
//!   them back, so the store sees ADR 0011 §5's layout.
//! - Removing moves the file to the volume's trash, `kai-fs 1`'s only delete.

use crate::store::{OBJECTS, PARTIAL, Volume};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use fsproto::{Error, How, Reply, Request};
use kabi::handle::Rights;

/// One `kai-fs 1` exchange.
pub trait Transport {
    type Handle: Copy;
    /// `request` on handle `on`: the reply, and the handle it moved.
    fn call(
        &mut self,
        on: Self::Handle,
        request: Request,
    ) -> Result<(Reply, Option<Self::Handle>), String>;
    fn close(&mut self, handle: Self::Handle);
}

/// Where the partial downloads lie beside the objects (see the module doc).
const PARTIAL_PREFIX: &str = ".partial.";

pub struct FsVolume<T: Transport> {
    pub transport: T,
    root: T::Handle,
    in_action: bool,
}

fn dir_rights() -> u32 {
    (Rights::READ | Rights::WRITE | Rights::ENUMERATE | Rights::CREATE | Rights::DELETE).bits()
}

fn file_rights() -> u32 {
    (Rights::READ | Rights::WRITE).bits()
}

/// `path` beneath `/system/pkg`, the store's `pkg/` taken off and a partial
/// download's folder folded into the store's.
fn local(path: &str) -> Result<String, String> {
    if let Some(name) = path.strip_prefix(PARTIAL).and_then(|r| r.strip_prefix('/')) {
        return Ok(format!("store/{PARTIAL_PREFIX}{name}"));
    }
    path.strip_prefix("pkg/")
        .map(String::from)
        .ok_or_else(|| format!("{path} is not beneath pkg/"))
}

/// The folder of `path` and its last name.
fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

impl<T: Transport> FsVolume<T> {
    /// Over `root`, the `Dir` handle for `/system/pkg`.
    pub fn new(transport: T, root: T::Handle) -> Self {
        FsVolume {
            transport,
            root,
            in_action: false,
        }
    }

    fn ask(
        &mut self,
        on: T::Handle,
        request: Request,
    ) -> Result<(Reply, Option<T::Handle>), String> {
        // The request's name only: its fields may be a file's bytes, such as
        // the device key's seed.
        let what = format!("{request:?}");
        let what = String::from(
            what.split(|c: char| !c.is_ascii_alphanumeric())
                .next()
                .unwrap_or(""),
        );
        self.transport
            .call(on, request)
            .map_err(|e| format!("{what}: {e}"))
    }

    /// `request` on `on`, which must answer Done.
    fn done(&mut self, on: T::Handle, request: Request) -> Result<(), String> {
        match self.ask(on, request)? {
            (Reply::Done, _) => Ok(()),
            (other, _) => Err(format!("fs answered {other:?}")),
        }
    }

    /// The object at `path` beneath the root, opened `how`; None if it is not
    /// there and `how` is [`How::Existing`].
    fn open(&mut self, path: &str, how: How, rights: u32) -> Result<Option<T::Handle>, String> {
        let request = Request::Open {
            path: path.into(),
            rights,
            how,
        };
        match self.ask(self.root, request)? {
            (Reply::Opened(..), Some(handle)) => Ok(Some(handle)),
            (Reply::Failed(Error::NotFound), _) if how == How::Existing => Ok(None),
            (other, _) => Err(format!("opening {path}: {other:?}")),
        }
    }

    /// Makes each missing folder of `path`'s parent, outermost first.
    fn folders(&mut self, path: &str) -> Result<(), String> {
        let (parent, _) = split(path);
        let mut at = 0;
        while at < parent.len() {
            let end = parent[at..].find('/').map_or(parent.len(), |i| at + i);
            let folder = &parent[..end];
            let handle = match self.open(folder, How::Existing, dir_rights())? {
                Some(handle) => handle,
                None => self
                    .open(folder, How::Mkdir, dir_rights())?
                    .ok_or("a new folder came back without a handle")?,
            };
            self.transport.close(handle);
            at = end + 1;
        }
        Ok(())
    }

    /// The file at `path`, made if it is not there.
    fn file(&mut self, path: &str) -> Result<T::Handle, String> {
        self.folders(path)?;
        match self.open(path, How::Existing, file_rights())? {
            Some(handle) => Ok(handle),
            None => self
                .open(path, How::CreateFile, file_rights())?
                .ok_or_else(|| format!("{path} came back without a handle")),
        }
    }

    /// The whole file `file` is a handle to, at most `max` bytes: a file
    /// another process moved to `pkg`, such as the artifact `cc build` wrote,
    /// read through that handle and nothing else.
    pub fn read_handle(&mut self, file: T::Handle, max: u64) -> Result<Vec<u8>, String> {
        let size = self.size(file)?;
        if size > max {
            return Err(format!("the file is {size} bytes, over {max}"));
        }
        let mut out = Vec::with_capacity(size as usize);
        while (out.len() as u64) < size {
            let want = (size - out.len() as u64).min(fsproto::MAX_DATA as u64) as u32;
            let request = Request::Read {
                off: out.len() as u64,
                len: want,
            };
            match self.ask(file, request)? {
                (Reply::Data(data), _) if !data.is_empty() => out.extend_from_slice(&data),
                (other, _) => return Err(format!("{other:?}")),
            }
        }
        Ok(out)
    }

    fn size(&mut self, file: T::Handle) -> Result<u64, String> {
        match self.ask(file, Request::Stat)? {
            (Reply::Stat(stat), _) => Ok(stat.size),
            (other, _) => Err(format!("a stat gave {other:?}")),
        }
    }

    /// `bytes` into `file` from `off`, in `kai-fs 1`'s pieces.
    fn put(&mut self, file: T::Handle, mut off: u64, bytes: &[u8]) -> Result<(), String> {
        for piece in bytes.chunks(fsproto::MAX_DATA) {
            let request = Request::Write {
                off,
                data: piece.to_vec(),
            };
            self.done(file, request)?;
            off += piece.len() as u64;
        }
        Ok(())
    }

    /// The names in folder `dir` beneath the root, as `kai-fs 1` lists them.
    fn names(&mut self, dir: &str) -> Result<Vec<String>, String> {
        let Some(handle) = self.open(dir, How::Existing, dir_rights())? else {
            return Ok(Vec::new());
        };
        let mut names = Vec::new();
        let mut after = None;
        let result = loop {
            let request = Request::List {
                after: after.take(),
                max: fsproto::MAX_LIST,
            };
            match self.ask(handle, request) {
                Ok((Reply::Listed { names: rows, more }, _)) => {
                    names.extend(rows.into_iter().map(|row| row.name));
                    match more {
                        Some(cursor) => after = Some(cursor),
                        None => break Ok(names),
                    }
                }
                Ok((other, _)) => break Err(format!("listing {dir}: {other:?}")),
                Err(e) => break Err(e),
            }
        };
        self.transport.close(handle);
        result
    }

    /// Runs `f` on the file at `path`, then closes it.
    fn with_file<R>(
        &mut self,
        path: &str,
        f: impl FnOnce(&mut Self, T::Handle) -> Result<R, String>,
    ) -> Result<R, String> {
        let file = self.file(path)?;
        let result = f(self, file);
        self.transport.close(file);
        result
    }
}

impl<T: Transport> Volume for FsVolume<T> {
    fn begin(&mut self, action: u64) -> Result<(), String> {
        self.done(self.root, Request::Begin { action })?;
        self.in_action = true;
        Ok(())
    }

    fn abort(&mut self) -> Result<(), String> {
        match core::mem::take(&mut self.in_action) {
            true => self.done(self.root, Request::Abort),
            false => Ok(()),
        }
    }

    fn commit(&mut self) -> Result<(), String> {
        let request = match core::mem::take(&mut self.in_action) {
            true => Request::Commit,
            false => Request::Sync,
        };
        match self.ask(self.root, request)? {
            (Reply::Committed { .. }, _) => Ok(()),
            (other, _) => Err(format!("a commit gave {other:?}")),
        }
    }

    fn read(&mut self, path: &str) -> Result<Option<Vec<u8>>, String> {
        let path = local(path)?;
        let Some(file) = self.open(&path, How::Existing, Rights::READ.bits())? else {
            return Ok(None);
        };
        let result = self
            .read_handle(file, u64::MAX)
            .map_err(|why| format!("reading {path}: {why}"));
        self.transport.close(file);
        result.map(Some)
    }

    fn list(&mut self, dir: &str) -> Result<Vec<String>, String> {
        if dir == PARTIAL {
            let names = self.names("store")?;
            return Ok(names
                .iter()
                .filter_map(|n| n.strip_prefix(PARTIAL_PREFIX).map(String::from))
                .collect());
        }
        let names = self.names(&local(dir)?)?;
        Ok(match dir == OBJECTS {
            true => names
                .into_iter()
                .filter(|n| !n.starts_with(PARTIAL_PREFIX))
                .collect(),
            false => names,
        })
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let path = local(path)?;
        self.with_file(&path, |vol, file| {
            vol.done(file, Request::Truncate { size: 0 })?;
            vol.put(file, 0, bytes)
        })
    }

    fn append(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let path = local(path)?;
        self.with_file(&path, |vol, file| {
            let end = vol.size(file)?;
            vol.put(file, end, bytes)
        })
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        let (from, to) = (local(from)?, local(to)?);
        let ((dir, old), (to_dir, new)) = (split(&from), split(&to));
        if dir != to_dir {
            return Err(format!("fs renames within one folder, not {from} to {to}"));
        }
        let folder = match dir {
            "" => self.root,
            dir => self
                .open(dir, How::Existing, dir_rights())?
                .ok_or_else(|| format!("no folder {dir}"))?,
        };
        let result = (|| {
            // `kai-fs 1` refuses a taken name; the store's rename replaces it.
            match self.ask(folder, Request::Trash { name: new.into() })? {
                (Reply::Done | Reply::Failed(Error::NotFound), _) => {}
                (other, _) => return Err(format!("clearing {to}: {other:?}")),
            }
            self.done(
                folder,
                Request::Rename {
                    from: old.into(),
                    to: new.into(),
                },
            )
        })();
        if !dir.is_empty() {
            self.transport.close(folder);
        }
        result
    }

    fn remove(&mut self, path: &str) -> Result<(), String> {
        let path = local(path)?;
        let (dir, name) = split(&path);
        let folder = match dir {
            "" => self.root,
            dir => match self.open(dir, How::Existing, dir_rights())? {
                Some(handle) => handle,
                None => return Ok(()),
            },
        };
        let result = match self.ask(folder, Request::Trash { name: name.into() }) {
            Ok((Reply::Done | Reply::Failed(Error::NotFound), _)) => Ok(()),
            Ok((other, _)) => Err(format!("removing {path}: {other:?}")),
            Err(e) => Err(e),
        };
        if !dir.is_empty() {
            self.transport.close(folder);
        }
        result
    }
}
