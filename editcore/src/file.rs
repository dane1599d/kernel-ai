//! A file of the person's own, edited with the commands and the screen that a
//! component's source uses (the editor on the default boot, P4 task 11).
//!
//! [`FileEditor`] answers the screen's requests itself, in the call that makes
//! them, from a [`Session`] over the one file: the edit service is for
//! components and their builds, and a file needs neither. What it adds is the
//! file's bytes ([`FileEditor::bytes`]), which the app saves through the OS, and
//! [`FileEditor::saved`], which tells it they were. Nothing here reads or writes
//! a file: the OS does, through its own `fs_*` rows, so tiers, the log and undo
//! are the OS's.
//!
//! The session is the service's, so every command, view and undo works as it
//! does there. It names the file without its leading `/` (a bundle's paths are
//! relative), and calls it component `file`, version `disk`, which the screen
//! shows in its title. `build` and `try` have nothing to build and say so.

use crate::MAX_BUNDLE;
use crate::command::{self, Command};
use crate::ipc::{self, Ipc};
use crate::service::{Env, Service};
use crate::session::Session;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::Info;
use livecore::artifact;

/// What the screen is told the "component" of a file is.
pub const COMPONENT: &str = "file";
/// And its "version": the file as the disk has it.
pub const LABEL: &str = "disk";
/// The most bytes of a file the editor opens: the session's bundle limit, less
/// its headers.
pub const MAX_BYTES: usize = MAX_BUNDLE - 512;
/// Why `build` and `try` do nothing.
pub const NOTHING_TO_BUILD: &str = "a file is saved with Ctrl-S; there is nothing to build here";
/// Longest path a file request names (`kai:files`).
pub const MAX_PATH: usize = 255;

/// Why `path` is not one the OS takes (`kai:files`): absolute, plain segments, no
/// whitespace, colon, backslash or control character, at most [`MAX_PATH`] bytes.
/// The app says so at the prompt, so a bad path never leaves the editor.
pub fn check_path(path: &str) -> Result<(), String> {
    let plain = path
        .strip_prefix('/')
        .ok_or_else(|| format!("{path:?} is not an absolute path, such as /home/user/notes.txt"))?;
    if path.len() > MAX_PATH {
        return Err(format!("the path is over {MAX_PATH} bytes"));
    }
    artifact::valid_path(plain).map_err(|why| format!("{path}: {why}"))
}

/// An environment with nothing in it: a file reads no component and builds no
/// artifact. The service asks it only for what `build` and `try` would need,
/// which [`FileEditor::handle`] refuses first.
struct NoEnv;

impl Env for NoEnv {
    fn info(&mut self, _component: &str) -> Result<Info, Error> {
        Err(Error::NotFound)
    }
    fn artifact(&mut self, _component: &str, _label: &str) -> Result<Vec<u8>, Error> {
        Err(Error::NotFound)
    }
    fn put(&mut self, _: &str, _: &[u8], _: &str, _: &str) -> Result<(), Error> {
        Err(Error::NotFound)
    }
    fn build(&mut self, _request: &[u8], _timeout_ms: u64) -> Result<Vec<u8>, Error> {
        Err(Error::NotFound)
    }
    fn build_start(&mut self, _request: &[u8], _timeout_ms: u64) -> Result<(), Error> {
        Err(Error::NotFound)
    }
    fn build_poll(&mut self, _take: bool) -> Result<Vec<u8>, Error> {
        Err(Error::NotFound)
    }
    fn control(&mut self, _component: &str, _request: &str) -> Result<Vec<u8>, Error> {
        Err(Error::NotFound)
    }
    fn log(&mut self, _text: &str) {}
}

fn err(text: impl AsRef<str>) -> String {
    format!("err {}", text.as_ref())
}

/// One file under edit.
pub struct FileEditor {
    service: Service,
    /// The person's path, with its leading `/`.
    path: String,
}

impl FileEditor {
    /// Opens `bytes` as the file at `path`. Why not: the path has a space or a
    /// colon, the file is over [`MAX_BYTES`] or 4,000 lines, or it is not text.
    pub fn open(path: &str, bytes: &[u8]) -> Result<Self, String> {
        let name = Self::name_of(path);
        artifact::valid_path(&name).map_err(|why| format!("{path}: {why}"))?;
        if bytes.len() > MAX_BYTES {
            return Err(format!(
                "{path} is over {} KiB, which the editor opens",
                MAX_BYTES / 1024
            ));
        }
        let bundle = artifact::bundle(&[(name.as_str(), bytes)]);
        let session = Session::open(COMPONENT, LABEL, &bundle)?;
        let mut service = Service::new("", "");
        service.session = Some(session);
        Ok(Self {
            service,
            path: String::from(path),
        })
    }

    /// The session's name for `path`: a bundle's paths are relative.
    pub fn name_of(path: &str) -> String {
        String::from(path.strip_prefix('/').unwrap_or(path))
    }

    /// The person's path.
    pub fn path(&self) -> &str {
        &self.path
    }

    fn session(&self) -> &Session {
        self.service
            .session
            .as_ref()
            .expect("a file editor always has its session")
    }

    /// The file as it stands, byte for byte except where a line was edited.
    pub fn bytes(&self) -> Vec<u8> {
        self.session().current().bytes()
    }

    /// Whether the file differs from the disk's (as opened or last saved).
    pub fn changed(&self) -> bool {
        self.session().changed()
    }

    /// The OS has written [`FileEditor::bytes`]: they are the disk's now.
    pub fn saved(&mut self) {
        if let Some(session) = self.service.session.as_mut() {
            session.keep();
        }
    }

    /// Answers one request of the screen's, as the edit service would.
    pub fn handle(&mut self, request: &[u8]) -> String {
        match ipc::decode_ipc(request) {
            Err(why) => err(why),
            Ok(Ipc::Open { at, .. }) => self.opened(at),
            Ok(Ipc::Cmd(line)) => match command::parse(line) {
                Ok(Command::Build | Command::Try) => err(NOTHING_TO_BUILD),
                _ => self.service.handle(&mut NoEnv, 0, request),
            },
            Ok(Ipc::Sync { .. } | Ipc::Replay { .. }) => err("a file has no builds to sync"),
        }
    }

    /// `open`: the file is the session already; the cursor goes to `at`.
    fn opened(&mut self, at: Option<ipc::At<'_>>) -> String {
        let session = self
            .service
            .session
            .as_mut()
            .expect("a file editor always has its session");
        let state = if session.changed() {
            ", with your edits"
        } else {
            ""
        };
        match at {
            Some(at) if session.goto(at.path, at.line as usize) => format!(
                "ok editing {COMPONENT} {LABEL} at {}:{}{state}",
                at.path,
                session.line()
            ),
            Some(at) => err(format!("{} is not the file open", at.path)),
            None => format!(
                "ok editing {COMPONENT} {LABEL}: {}, {} lines{state}",
                session.current().path,
                session.current().len()
            ),
        }
    }
}
