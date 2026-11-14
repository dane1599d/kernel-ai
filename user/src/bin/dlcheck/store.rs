//! A download's bytes in kaifs, through the `fs` service (`kai-fs 1`): the
//! [`Partial`] `kainet::download` writes to.
//!
//! A target `<name>` grows as `<name>.partial` in the downloads folder, beside
//! `<name>.mark`, which holds how many chunks are checked and written, in
//! decimal. Each chunk and its mark are one action, so a cut or a reboot
//! leaves both as they were after the last whole chunk: the mark never names
//! a chunk that is not on disk. `finish` renames the partial to `<name>`. fs
//! renames only within one folder, so the partial sits beside its target
//! rather than in a `.partial/` folder of its own. A file is made at the first
//! write, so a download refused before its first chunk leaves nothing.

use alloc::format;
use alloc::string::String;
use fsproto::{Error, How, Reply, Request};
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, MAX_TRANSFER, Rights};
use kainet::download::{DownloadError, Partial};
use user::sys;

const CALL_MS: u64 = 30_000;

/// The `Dir` handle FILES gave at startup: `/home/user`.
pub fn home() -> Option<Handle> {
    let mut handles = [HandleInfo::default(); MAX_STARTUP];
    let count = sys::startup(&mut handles).ok()?;
    handles[..count.min(MAX_STARTUP)]
        .iter()
        .find(|h| h.kind() == Some(Kind::Dir))
        .map(|h| Handle(h.handle))
}

/// One request on `on`: the reply, and the handle it moved, if any.
pub fn ask(on: Handle, request: Request) -> Result<(Reply, Option<Handle>), String> {
    let bytes = fsproto::encode_request(&request);
    let mut reply = alloc::vec![0u8; fsproto::MAX_MESSAGE];
    let mut moved = [Handle::NONE; MAX_TRANSFER];
    let (len, came) = sys::call_h(on, &bytes, &mut reply, CALL_MS, &[], &mut moved)
        .map_err(|e| format!("CALL_H: {e:?}"))?;
    let reply = fsproto::decode_reply(&reply[..len]).map_err(|e| format!("a reply: {e:?}"))?;
    Ok((reply, (came > 0).then_some(moved[0])))
}

fn done(on: Handle, request: Request) -> Result<(), String> {
    let what = format!("{request:?}");
    match ask(on, request)? {
        (Reply::Done, _) => Ok(()),
        (other, _) => Err(format!("{what}: {other:?}")),
    }
}

/// Opens `path` under `dir` as `how` says: its handle, or `None` if it is
/// not there.
pub fn open(dir: Handle, path: &str, how: How, rights: Rights) -> Result<Option<Handle>, String> {
    let request = Request::Open {
        path: path.into(),
        rights: rights.bits(),
        how,
    };
    match ask(dir, request)? {
        (Reply::Opened(..), Some(handle)) => Ok(Some(handle)),
        (Reply::Failed(Error::NotFound), _) if how == How::Existing => Ok(None),
        other => Err(format!("opening {path}: {other:?}")),
    }
}

/// The folder `name` under `home`, made if it is not there.
pub fn folder(home: Handle, name: &str) -> Result<Handle, String> {
    let rights = Rights::READ | Rights::WRITE | Rights::ENUMERATE | Rights::CREATE | Rights::DELETE;
    if let Some(dir) = open(home, name, How::Existing, rights)? {
        return Ok(dir);
    }
    open(home, name, How::Mkdir, rights)?.ok_or_else(|| format!("{name} was not made"))
}

/// The size of `name` under `dir`, or `None` if it is not there.
pub fn size(dir: Handle, name: &str) -> Result<Option<u64>, String> {
    let Some(file) = open(dir, name, How::Existing, Rights::READ)? else {
        return Ok(None);
    };
    let stat = ask(file, Request::Stat);
    let _ = sys::handle_close(file);
    match stat? {
        (Reply::Stat(stat), _) => Ok(Some(stat.size)),
        (other, _) => Err(format!("{name}'s stat: {other:?}")),
    }
}

/// Reads `buf.len()` bytes of `file` from `off`.
fn read_at(file: Handle, off: u64, buf: &mut [u8]) -> Result<(), String> {
    let mut at = 0;
    while at < buf.len() {
        let len = (buf.len() - at).min(fsproto::MAX_DATA) as u32;
        let request = Request::Read {
            off: off + at as u64,
            len,
        };
        match ask(file, request)? {
            (Reply::Data(data), _) if !data.is_empty() && data.len() <= buf.len() - at => {
                buf[at..at + data.len()].copy_from_slice(&data);
                at += data.len();
            }
            other => return Err(format!("a read at {}: {other:?}", off + at as u64)),
        }
    }
    Ok(())
}

/// Writes `bytes` to `file` from `off`, a message at a time.
fn write_at(file: Handle, off: u64, bytes: &[u8]) -> Result<(), String> {
    for (i, piece) in bytes.chunks(fsproto::MAX_DATA).enumerate() {
        let request = Request::Write {
            off: off + (i * fsproto::MAX_DATA) as u64,
            data: piece.to_vec(),
        };
        done(file, request)?;
    }
    Ok(())
}

/// How many chunks of `name` its mark says are checked and written: 0 if it
/// has none.
pub fn marked(dir: Handle, name: &str) -> Result<usize, String> {
    let mut partial = Kaifs::new(dir, name);
    let got = partial.read_mark();
    partial.close();
    got
}

/// A download's partial target and its mark, in `dir`.
pub struct Kaifs {
    dir: Handle,
    name: String,
    file: Option<Handle>,
    mark: Option<Handle>,
    /// The action open since the last chunk's write, closed by its mark.
    open: bool,
    actions: u64,
    /// The last error, in words: [`DownloadError::Store`] takes only a
    /// static text.
    pub why: Option<String>,
}

impl Kaifs {
    pub fn new(dir: Handle, name: &str) -> Self {
        Self {
            dir,
            name: name.into(),
            file: None,
            mark: None,
            open: false,
            actions: 0,
            why: None,
        }
    }

    pub fn partial_name(&self) -> String {
        format!("{}.partial", self.name)
    }

    pub fn mark_name(&self) -> String {
        format!("{}.mark", self.name)
    }

    fn failed(&mut self, why: String, what: &'static str) -> DownloadError {
        self.why = Some(why);
        DownloadError::Store(what)
    }

    /// The handle for `name`, opened or made at first use.
    fn handle(&mut self, mark: bool, make: bool) -> Result<Option<Handle>, String> {
        let held = if mark { self.mark } else { self.file };
        if held.is_some() {
            return Ok(held);
        }
        let name = if mark {
            self.mark_name()
        } else {
            self.partial_name()
        };
        let rights = Rights::READ | Rights::WRITE;
        let mut got = open(self.dir, &name, How::Existing, rights)?;
        if got.is_none() && make {
            got = open(self.dir, &name, How::CreateFile, rights)?;
        }
        if mark {
            self.mark = got;
        } else {
            self.file = got;
        }
        Ok(got)
    }

    fn begin(&mut self) -> Result<(), String> {
        if !self.open {
            self.actions += 1;
            done(
                self.dir,
                Request::Begin {
                    action: self.actions,
                },
            )?;
            self.open = true;
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<(), String> {
        self.open = false;
        match ask(self.dir, Request::Commit)? {
            (Reply::Committed { .. }, _) => Ok(()),
            (other, _) => Err(format!("a commit: {other:?}")),
        }
    }

    fn read_mark(&mut self) -> Result<usize, String> {
        let Some(mark) = self.handle(true, false)? else {
            return Ok(0);
        };
        let size = match ask(mark, Request::Stat)? {
            (Reply::Stat(stat), _) => stat.size as usize,
            (other, _) => return Err(format!("the mark's stat: {other:?}")),
        };
        let mut text = alloc::vec![0u8; size.min(20)];
        read_at(mark, 0, &mut text)?;
        core::str::from_utf8(&text)
            .ok()
            .and_then(|t| t.trim().parse().ok())
            .ok_or_else(|| String::from("the mark is not a number"))
    }

    fn write_mark(&mut self, done: usize) -> Result<(), String> {
        self.begin()?;
        let mark = self
            .handle(true, true)?
            .ok_or_else(|| String::from("no mark"))?;
        // Twenty digits, so the mark's size never changes after its first write.
        let text = format!("{done:020}");
        write_at(mark, 0, text.as_bytes())?;
        self.commit()
    }

    /// Closes what it holds; an action left open is dropped.
    pub fn close(&mut self) {
        if self.open {
            let _ = ask(self.dir, Request::Abort);
            self.open = false;
        }
        for handle in [self.file.take(), self.mark.take()].into_iter().flatten() {
            let _ = sys::handle_close(handle);
        }
    }
}

impl Partial for Kaifs {
    fn checkpoint(&mut self) -> Result<usize, DownloadError> {
        self.read_mark().map_err(|e| self.failed(e, "the mark"))
    }

    fn record(&mut self, done: usize) -> Result<(), DownloadError> {
        self.write_mark(done)
            .map_err(|e| self.failed(e, "the mark"))
    }

    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DownloadError> {
        let result = self.begin().and_then(|()| {
            let file = self
                .handle(false, true)?
                .ok_or_else(|| String::from("no partial"))?;
            write_at(file, offset, bytes)
        });
        result.map_err(|e| self.failed(e, "a write"))
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), DownloadError> {
        let result = self.handle(false, false).and_then(|file| {
            let file = file.ok_or_else(|| String::from("no partial to read"))?;
            read_at(file, offset, buf)
        });
        result.map_err(|e| self.failed(e, "a read"))
    }

    /// The partial's length, by `Stat` on its handle: what `download` holds
    /// to the target's before it moves the partial in.
    fn size(&mut self) -> Result<u64, DownloadError> {
        let result = self.handle(false, false).and_then(|file| {
            let file = file.ok_or_else(|| String::from("no partial"))?;
            match ask(file, Request::Stat)? {
                (Reply::Stat(stat), _) => Ok(stat.size),
                (other, _) => Err(format!("the partial's stat: {other:?}")),
            }
        });
        result.map_err(|e| self.failed(e, "the partial's length"))
    }

    fn finish(&mut self) -> Result<(), DownloadError> {
        // The partial's handle goes first: a rename of an open file is fine
        // in kaifs, but nothing more is written through it.
        if let Some(file) = self.file.take() {
            let _ = sys::handle_close(file);
        }
        let request = Request::Rename {
            from: self.partial_name(),
            to: self.name.clone(),
        };
        done(self.dir, request).map_err(|e| self.failed(e, "the rename"))
    }
}
