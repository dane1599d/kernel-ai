//! `/system/state`: the shell's own files (its action log, standing rules,
//! refused rules files kept aside), beside the user's files that
//! [`super::Files`] reaches.

use super::{ask, close, expect_done, open, read_all, unexpected, write_all};
use alloc::format;
use alloc::string::{String, ToString};
use fsproto::{Error, How, Reply, Request};
use kabi::handle::{Handle, Rights};
use shellcore::FsRefused;

/// `/system/state`, where the shell keeps its action log and standing rules
/// (P2 task 12). The AI's file rows never reach it: they go through `Files`'
/// home `Dir` only.
pub struct State {
    dir: Option<Handle>,
}

/// The action log's file beneath `/system/state`.
pub const LOG: &str = "log";
/// The standing rules' file: rule lines only, never grants, until `powerbox`
/// owns `/system/grants`.
pub const RULES: &str = "rules";
/// The standing instructions' store (`kai-standing 1`, P3 task 11).
pub const STANDING: &str = "standing";
/// The apps kept running, never napped (`kai-keep 1`, P4).
pub const KEEP: &str = "keep";
/// The user's appearance (`kai-appearance 1`, P4 task 8).
pub const APPEARANCE: &str = "appearance";
/// The rows withheld from apps' effects (`kai-withheld 1`, P4 task 7).
pub const WITHHELD: &str = "withheld";
/// The apps' network grants (`kai-grants 1` host lines, P5 task 13).
pub const NETGRANTS: &str = "netgrants";
/// The restart window (`kai-updates 1`), in the folder `updated`'s read-only
/// `dir=restart` gives it alone.
pub const RESTART: &str = "restart/window";
const _: () = {
    let (path, folder, file) = (
        RESTART.as_bytes(),
        bootslot::window::FOLDER.as_bytes(),
        bootslot::window::FILE.as_bytes(),
    );
    assert!(path.len() == folder.len() + 1 + file.len());
    let mut i = 0;
    while i < folder.len() {
        assert!(path[i] == folder[i]);
        i += 1;
    }
    assert!(path[folder.len()] == b'/');
    let mut j = 0;
    while j < file.len() {
        assert!(path[folder.len() + 1 + j] == file[j]);
        j += 1;
    }
};

impl State {
    /// `/system/state` through `dir`, the second `Dir` STARTUP lists.
    pub(super) fn new(dir: Option<Handle>) -> Self {
        Self { dir }
    }

    /// File `name` as `/system/state` holds it; `None` only with no such file
    /// or no handle (nothing was ever written there), and why not for any other
    /// failure: a full disk or an fs error is no proof the file is absent.
    pub fn read(&self, name: &str) -> Result<Option<String>, String> {
        let Some(dir) = self.dir else {
            return Ok(None);
        };
        let request = Request::Open {
            path: name.to_string(),
            rights: Rights::READ.bits(),
            how: How::Existing,
        };
        let file = match ask(dir, request)? {
            (Reply::Opened(..), Some(handle)) => handle,
            (Reply::Failed(Error::NotFound), _) => return Ok(None),
            (other, _) => return Err(format!("{name}: {}", unexpected(&other))),
        };
        let text = read_all(file);
        close(file);
        text.map(Some)
    }

    /// File `name` and when it was last written (nanoseconds since 1970), as
    /// [`State::read`] reads it.
    pub fn read_kept(&self, name: &str) -> Result<Option<(String, u64)>, String> {
        let Some(dir) = self.dir else {
            return Ok(None);
        };
        let request = Request::Open {
            path: name.to_string(),
            rights: Rights::READ.bits(),
            how: How::Existing,
        };
        let (file, mtime) = match ask(dir, request)? {
            (Reply::Opened(stat, _), Some(handle)) => (handle, stat.mtime),
            (Reply::Failed(Error::NotFound), _) => return Ok(None),
            (other, _) => return Err(format!("{name}: {}", unexpected(&other))),
        };
        let text = read_all(file);
        close(file);
        text.map(|text| Some((text, mtime)))
    }

    /// Moves file `name` to the trash, as one action of its own: the
    /// generation it committed as.
    pub fn trash(&self, name: &str, action: u64) -> Result<u64, String> {
        let dir = self
            .dir
            .ok_or_else(|| String::from("no /system/state here"))?;
        begin(dir, action)?;
        match expect_done(dir, Request::Trash { name: name.into() }) {
            Ok(()) => commit(dir),
            Err(why) => {
                let _ = ask(dir, Request::Abort);
                Err(why)
            }
        }
    }

    /// Moves the newest trashed file that was `name` back, as one action of
    /// its own ([`shellcore::untrash_on`], which aborts it on any failure).
    pub fn untrash(&self, name: &str, action: u64) -> Result<(), FsRefused> {
        let dir = self
            .dir
            .ok_or_else(|| FsRefused::Failed(String::from("no /system/state here")))?;
        shellcore::untrash_on(name, action, |request| {
            ask(dir, request).map(|(reply, _)| reply)
        })
    }

    /// Writes the whole of file `name` into the action now open, the one that
    /// changed files.
    pub fn write_in(&self, name: &str, text: &str) -> Result<(), String> {
        let dir = self
            .dir
            .ok_or_else(|| String::from("no /system/state here"))?;
        open(dir, name, How::Existing, Rights::WRITE)
            .or_else(|_| open(dir, name, How::CreateFile, Rights::WRITE))
            .and_then(|file| {
                let wrote = expect_done(file, Request::Truncate { size: 0 })
                    .and_then(|()| write_all(file, text.as_bytes()));
                close(file);
                wrote
            })
    }

    /// Writes the whole of file `name`, as one action of its own: a line that
    /// changed no files.
    pub fn write(&self, name: &str, text: &str, action: u64) -> Result<(), String> {
        let dir = self
            .dir
            .ok_or_else(|| String::from("no /system/state here"))?;
        begin(dir, action)?;
        match self.write_in(name, text) {
            Ok(()) => commit(dir).map(|_| ()),
            Err(why) => {
                let _ = ask(dir, Request::Abort);
                Err(why)
            }
        }
    }
}

/// Begins an action of its own on `dir`. A Begin whose call got no answer
/// may still have reached `fs` and opened the action, which would answer
/// every later Begin of the shell's with Busy, so it is aborted; a refused
/// Begin opened none (#1673's review).
fn begin(dir: Handle, action: u64) -> Result<(), String> {
    match ask(dir, Request::Begin { action }) {
        Ok((Reply::Done, _)) => Ok(()),
        Ok((other, _)) => Err(unexpected(&other)),
        Err(why) => {
            let _ = ask(dir, Request::Abort);
            Err(why)
        }
    }
}

/// Commits the action open on `dir`: its generation. One that failed, or
/// whose call got no answer, is aborted, so it never stays open.
fn commit(dir: Handle) -> Result<u64, String> {
    let why = match ask(dir, Request::Commit) {
        Ok((Reply::Committed { generation }, _)) => return Ok(generation),
        Ok((other, _)) => unexpected(&other),
        Err(why) => why,
    };
    let _ = ask(dir, Request::Abort);
    Err(why)
}
