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
//! - A transport with a bulk `Shm` ([`Transport::bulk`]) moves a file's bytes
//!   up to the `Shm`'s length a round trip (ReadShm, WriteShm); without one, or
//!   with an `fs` that predates them, it goes a message at a time. The round
//!   trip, not `fs`, is what a 7 KiB message costs (#1101).

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
    /// Waits `ms` before a request `fs` answered Busy is asked again.
    fn wait_ms(&mut self, ms: u64);
    /// The bulk `Shm`, if this transport has one: none by default.
    fn bulk(&mut self) -> Option<&mut dyn Bulk<Self::Handle>> {
        None
    }
}

/// A bulk `Shm` and the requests that move it (`kai-fs 1`'s ReadShm and
/// WriteShm).
pub trait Bulk<H> {
    /// The `Shm`'s bytes: what a WriteShm sends from its start and a ReadShm
    /// fills from it.
    fn bytes(&mut self) -> &mut [u8];
    /// `request`, a ReadShm or WriteShm, on `on` with the `Shm` moved along.
    fn call(&mut self, on: H, request: Request) -> Result<Reply, String>;
    /// Whether the `Shm` may still be sent: false for good once a call failed
    /// after it moved (a timeout, `fs` gone), since that request may still be
    /// running in `fs` and could yet read or fill it. [`FsVolume`] then goes a
    /// message at a time.
    fn usable(&self) -> bool {
        true
    }
}

/// The transport's bulk `Shm`, if it has one it may still send.
fn usable<T: Transport>(transport: &mut T) -> Option<&mut dyn Bulk<T::Handle>> {
    transport.bulk().filter(|bulk| bulk.usable())
}

/// A reply, alone or with the handle it moved: what [`FsVolume::ask`]'s
/// retry looks at.
trait AsReply {
    fn reply(&self) -> &Reply;
}

impl AsReply for Reply {
    fn reply(&self) -> &Reply {
        self
    }
}

impl<H> AsReply for (Reply, Option<H>) {
    fn reply(&self) -> &Reply {
        &self.0
    }
}

/// How long to wait between asks while another caller's action holds `fs`.
pub const BUSY_WAIT_MS: u64 = 100;
/// How many times a Busy request is asked again: about 10 s in all, longer
/// than any one action at boot, after which Busy is the store's error.
pub const BUSY_TRIES: u32 = 100;

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

    /// `request` on `on`. While another caller's action holds `fs` it answers a
    /// change (or a Begin) Busy and leaves everything as it was, so the request
    /// is asked again, each [`BUSY_WAIT_MS`] up to [`BUSY_TRIES`] times: at boot
    /// the shell's first change can hold `fs` as `pkg` opens its store.
    fn ask(
        &mut self,
        on: T::Handle,
        request: Request,
    ) -> Result<(Reply, Option<T::Handle>), String> {
        self.retried(request, |transport, request| transport.call(on, request))
    }

    /// `request` sent by `send`, and sent again while `fs` answers it Busy, as
    /// [`FsVolume::ask`] says.
    fn retried<R: AsReply>(
        &mut self,
        request: Request,
        mut send: impl FnMut(&mut T, Request) -> Result<R, String>,
    ) -> Result<R, String> {
        // The request's name only: its fields may be a file's bytes, such as
        // the device key's seed.
        let what = format!("{request:?}");
        let what = String::from(
            what.split(|c: char| !c.is_ascii_alphanumeric())
                .next()
                .unwrap_or(""),
        );
        let mut tries = 0;
        loop {
            let answer =
                send(&mut self.transport, request.clone()).map_err(|e| format!("{what}: {e}"))?;
            if !matches!(answer.reply(), Reply::Failed(Error::Busy)) || tries == BUSY_TRIES {
                return Ok(answer);
            }
            tries += 1;
            self.transport.wait_ms(BUSY_WAIT_MS);
        }
    }

    /// `request` on `file` through the bulk `Shm`, sent again on Busy.
    fn ask_bulk(&mut self, file: T::Handle, request: Request) -> Result<Reply, String> {
        self.retried(request, |transport, request| match usable(transport) {
            Some(bulk) => bulk.call(file, request),
            None => Err("no bulk Shm".into()),
        })
    }

    /// The bulk `Shm`'s length, 0 without one.
    fn bulk_len(&mut self) -> usize {
        usable(&mut self.transport).map_or(0, |b| b.bytes().len())
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
        if size > fsproto::MAX_DATA as u64 {
            self.read_bulk(file, size, &mut out)?;
        }
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

    /// Reads `file` into `out`, up to `size` bytes, through the bulk `Shm` if
    /// there is one and `fs` takes ReadShm; the rest goes a message at a time.
    fn read_bulk(&mut self, file: T::Handle, size: u64, out: &mut Vec<u8>) -> Result<(), String> {
        let piece = self.bulk_len() as u64;
        while piece > 0 && (out.len() as u64) < size {
            let len = (size - out.len() as u64).min(piece) as u32;
            let request = Request::ReadShm {
                off: out.len() as u64,
                len,
            };
            match self.ask_bulk(file, request)? {
                Reply::Failed(Error::BadRequest) if out.is_empty() => return Ok(()),
                Reply::Copied { len: got } if got > 0 && got <= u64::from(len) => {
                    let bulk = self.transport.bulk().ok_or("no bulk Shm")?;
                    out.extend_from_slice(&bulk.bytes()[..got as usize]);
                }
                other => return Err(format!("a bulk read gave {other:?}")),
            }
        }
        Ok(())
    }

    fn size(&mut self, file: T::Handle) -> Result<u64, String> {
        match self.ask(file, Request::Stat)? {
            (Reply::Stat(stat), _) => Ok(stat.size),
            (other, _) => Err(format!("a stat gave {other:?}")),
        }
    }

    /// `bytes` into `file` from `off`: through the bulk `Shm` if there is one
    /// and `fs` takes WriteShm, else in `kai-fs 1`'s message pieces.
    fn put(&mut self, file: T::Handle, mut off: u64, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > fsproto::MAX_DATA && self.put_bulk(file, off, bytes)? {
            return Ok(());
        }
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

    /// [`FsVolume::put`] through the bulk `Shm`: `false`, with nothing
    /// written, when there is none or `fs` predates WriteShm.
    fn put_bulk(&mut self, file: T::Handle, off: u64, bytes: &[u8]) -> Result<bool, String> {
        let size = self.bulk_len();
        if size == 0 {
            return Ok(false);
        }
        for (i, piece) in bytes.chunks(size).enumerate() {
            let bulk = self.transport.bulk().ok_or("no bulk Shm")?;
            bulk.bytes()[..piece.len()].copy_from_slice(piece);
            let request = Request::WriteShm {
                off: off + (i * size) as u64,
                len: piece.len() as u32,
            };
            match self.ask_bulk(file, request)? {
                Reply::Done => {}
                Reply::Failed(Error::BadRequest) if i == 0 => return Ok(false),
                other => return Err(format!("a bulk write gave {other:?}")),
            }
        }
        Ok(true)
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
