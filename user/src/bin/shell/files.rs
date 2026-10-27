//! The shell's `fs` client (P2 task 11): the file rows' host calls through the
//! `Dir` handle for `/home/user` the shell gets at startup with `files`, in
//! `kai-fs 1`. A line's changes are one action (`Begin` … `Commit`), left open
//! until the log line that records them is written into it (P2 task 12), so the
//! change and its log line land in one commit. The commit's generation, known at
//! `Begin` (`Request::Generation`), is its transaction for `undo`; `undo` is a
//! `Restore` of it, never over a later change.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;
use fsproto::{Error, How, Outcome, Reply, Request};
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, MAX_TRANSFER, Rights};
use shellcore::{FsChange, FsRead, FsRefused};
use user::sys;

const CALL_MS: u64 = 10_000;
/// The most of a file `fs_read` hands a script: a few messages' worth.
const READ_MAX: usize = 4 * fsproto::MAX_DATA;
/// The most names `fs_list` gives.
const LIST_MAX: usize = 200;

/// `/home/user`, if the shell was started with `files`.
pub struct Files {
    home: Option<Handle>,
    /// The next action number `Begin` gets.
    action: Cell<u64>,
    /// The generation of the line's open action, until the log commits it.
    open: Cell<Option<u64>>,
    /// An open action dropped by a failed run's rollback or by `fs`: its
    /// generation, which is nothing to restore.
    dropped: Cell<Option<u64>>,
}

impl Files {
    /// `/home/user` and `/system/state`: STARTUP lists FILES's `Dir` before
    /// SYSTEM_STATE's (the kernel's `handle::create`).
    pub fn new() -> (Self, State) {
        let dirs = startup_dirs();
        let files = Self {
            home: dirs.first().copied(),
            action: Cell::new(1),
            open: Cell::new(None),
            dropped: Cell::new(None),
        };
        let state = State {
            dir: dirs.get(1).copied(),
        };
        (files, state)
    }

    fn home(&self) -> Result<Handle, String> {
        self.home
            .ok_or_else(|| String::from("there are no files here (no kai-data disk)"))
    }

    pub fn read(&self, read: FsRead) -> Result<String, String> {
        let home = self.home()?;
        match read {
            FsRead::List(path) => {
                in_dir(home, path, Rights::READ | Rights::ENUMERATE, list).map(|names| {
                    names.join(
                        "
",
                    )
                })
            }
            FsRead::Read(path) => {
                let file = open(home, path, How::Existing, Rights::READ)?;
                let text = read_all(file);
                close(file);
                text
            }
            FsRead::History(path) => {
                let file = open(home, path, How::Existing, Rights::READ)?;
                let answer = ask(file, Request::History);
                close(file);
                match answer?.0 {
                    Reply::History(versions) => Ok(versions
                        .iter()
                        .map(|v| format!("version {}: {} bytes", v.generation, v.stat.size))
                        .collect::<Vec<_>>()
                        .join("\n")),
                    other => Err(unexpected(&other)),
                }
            }
            FsRead::Changes => match ask(home, Request::Changes { since: 0 })?.0 {
                Reply::Changes { rows, more } => Ok(format!(
                    "{} changed{}",
                    rows.len(),
                    if more { " (and more)" } else { "" }
                )),
                other => Err(unexpected(&other)),
            },
        }
    }

    /// The next action number, for `Begin`.
    pub fn next_action(&self) -> u64 {
        let action = self.action.get();
        self.action.set(action + 1);
        action
    }

    /// One change in the line's action, opened by its first change; the action's
    /// generation, the transaction `undo` restores.
    pub fn change(&self, change: FsChange) -> Result<u64, String> {
        let home = self.home()?;
        let generation = match self.open.get() {
            Some(generation) => generation,
            None => self.begin(home)?,
        };
        make(home, change).inspect_err(|why| {
            // A refused change leaves the action open; a failed disk drops it.
            if why.contains("Io") {
                self.open.set(None);
                self.dropped.set(Some(generation));
            }
        })?;
        Ok(generation)
    }

    fn begin(&self, home: Handle) -> Result<u64, String> {
        let action = self.next_action();
        expect_done(home, Request::Begin { action })?;
        let generation = match ask(home, Request::Generation)?.0 {
            Reply::Generation { generation } => generation,
            other => {
                let _ = ask(home, Request::Abort);
                return Err(unexpected(&other));
            }
        };
        self.open.set(Some(generation));
        self.dropped.set(None);
        Ok(generation)
    }

    /// Whether a line's changes wait for their log line.
    pub fn is_open(&self) -> bool {
        self.open.get().is_some()
    }

    /// Commits the line's action, its log line in it; an error if the commit got
    /// another generation than its changes were told.
    pub fn commit(&self) -> Result<(), String> {
        let (Some(generation), Ok(home)) = (self.open.take(), self.home()) else {
            return Ok(());
        };
        match ask(home, Request::Commit)?.0 {
            Reply::Committed { generation: got } if got == generation => Ok(()),
            Reply::Committed { generation: got } => Err(format!(
                "the files committed as {got}, not {generation}; undo may miss them"
            )),
            other => Err(unexpected(&other)),
        }
    }

    pub fn undo(&self, txn: u64, anyway: bool) -> Result<u64, FsRefused> {
        let home = self.home().map_err(FsRefused::Failed)?;
        // A failed run's rollback: its own action is dropped, not restored.
        if self.open.get() == Some(txn) {
            let _ = ask(home, Request::Abort);
            self.open.set(None);
            self.dropped.set(Some(txn));
            return Ok(txn);
        }
        if self.dropped.get() == Some(txn) {
            return Ok(txn);
        }
        // Another line's action still open (its log was not written) lands first.
        self.commit().map_err(FsRefused::Failed)?;
        let action = self.action.get();
        self.action.set(action + 1);
        let failed = |why: String| FsRefused::Failed(why);
        match ask(home, Request::Restore { txn, action })
            .map_err(failed)?
            .0
        {
            Reply::Restored { rows, generation } => {
                // `fs` never restores over a later change; it leaves those.
                let left = rows.iter().filter(|(_, o)| *o == Outcome::Left).count();
                match (left, anyway) {
                    (0, _) => Ok(generation),
                    (n, false) => Err(FsRefused::Later(format!(
                        "{n} file(s) changed since; fs leaves them"
                    ))),
                    (n, true) => Err(failed(format!(
                        "{n} file(s) changed since, and fs never restores over a later change"
                    ))),
                }
            }
            Reply::Failed(error) => Err(failed(describe(error))),
            other => Err(failed(unexpected(&other))),
        }
    }
}

/// The change itself, inside the open action.
fn make(home: Handle, change: FsChange) -> Result<(), String> {
    match change {
        FsChange::Create { path, text } => {
            let file = open(home, path, How::CreateFile, Rights::WRITE).map_err(|why| match why
                .contains("Exists")
            {
                true => format!("{path} exists; fs_create never overwrites, so use fs_write"),
                false => why,
            })?;
            let wrote = write_all(file, text.as_bytes());
            close(file);
            wrote
        }
        FsChange::Write { path, text } => {
            let file = open(home, path, How::Existing, Rights::WRITE)?;
            let wrote = expect_done(file, Request::Truncate { size: 0 })
                .and_then(|()| write_all(file, text.as_bytes()));
            close(file);
            wrote
        }
        FsChange::Move { from, to } => {
            let (from_dir, from_name) = parent(from);
            let (to_dir, to_name) = parent(to);
            if from_dir != to_dir {
                return Err(String::from(
                    "moving between folders comes later; rename within one",
                ));
            }
            in_dir(home, from_dir, Rights::DELETE | Rights::CREATE, |dir| {
                expect_done(
                    dir,
                    Request::Rename {
                        from: from_name.into(),
                        to: to_name.into(),
                    },
                )
            })
        }
        FsChange::Trash(path) => {
            let (dir_path, name) = parent(path);
            in_dir(home, dir_path, Rights::DELETE, |dir| {
                expect_done(dir, Request::Trash { name: name.into() })
            })
        }
        FsChange::Restore(_) | FsChange::EmptyTrash => Err(String::from(
            "restoring from the trash and emptying it come with the trash's rows in fs",
        )),
    }
}

/// `notes/plan.txt` as (`notes`, `plan.txt`); a bare name's folder is "".
fn parent(path: &str) -> (&str, &str) {
    let path = path.trim_matches('/');
    path.rsplit_once('/').unwrap_or(("", path))
}

fn startup_dirs() -> Vec<Handle> {
    let mut handles = [HandleInfo::default(); MAX_STARTUP];
    let count = sys::startup(&mut handles).unwrap_or(0);
    handles[..count.min(MAX_STARTUP)]
        .iter()
        .filter(|h| h.kind() == Some(Kind::Dir))
        .map(|h| Handle(h.handle))
        .collect()
}

/// `/system/state`, where the shell keeps its action log (P2 task 12). The AI's
/// file rows never reach it: they go through `Files`' home `Dir` only.
pub struct State {
    dir: Option<Handle>,
}

/// The log's file beneath `/system/state`.
const LOG: &str = "log";

impl State {
    /// The log as `/system/state/log` holds it; `None` with no such file or no
    /// handle.
    pub fn read_log(&self) -> Option<String> {
        let file = open(self.dir?, LOG, How::Existing, Rights::READ).ok()?;
        let text = read_all(file);
        close(file);
        text.ok()
    }

    /// Writes the whole log into the action now open, the one that changed files.
    pub fn write_log_in(&self, text: &str) -> Result<(), String> {
        let dir = self
            .dir
            .ok_or_else(|| String::from("no /system/state here"))?;
        open(dir, LOG, How::Existing, Rights::WRITE)
            .or_else(|_| open(dir, LOG, How::CreateFile, Rights::WRITE))
            .and_then(|file| {
                let wrote = expect_done(file, Request::Truncate { size: 0 })
                    .and_then(|()| write_all(file, text.as_bytes()));
                close(file);
                wrote
            })
    }

    /// Writes the whole log, as one action of its own: a line that changed no
    /// files.
    pub fn write_log(&self, text: &str, action: u64) -> Result<(), String> {
        let dir = self
            .dir
            .ok_or_else(|| String::from("no /system/state here"))?;
        expect_done(dir, Request::Begin { action })?;
        match self.write_log_in(text) {
            Ok(()) => match ask(dir, Request::Commit)?.0 {
                Reply::Committed { .. } => Ok(()),
                other => Err(unexpected(&other)),
            },
            Err(why) => {
                let _ = ask(dir, Request::Abort);
                Err(why)
            }
        }
    }
}

/// One request on `on`; the reply and the handle it moved, if any.
fn ask(on: Handle, request: Request) -> Result<(Reply, Option<Handle>), String> {
    let bytes = fsproto::encode_request(&request);
    let mut reply = vec![0u8; fsproto::MAX_MESSAGE];
    let mut moved = [Handle::NONE; MAX_TRANSFER];
    let (len, came) = sys::call_h(on, &bytes, &mut reply, CALL_MS, &[], &mut moved)
        .map_err(|e| format!("the fs service did not answer ({e:?})"))?;
    let reply = fsproto::decode_reply(&reply[..len.min(reply.len())])
        .map_err(|_| String::from("the fs service's answer was malformed"))?;
    Ok((reply, (came > 0).then_some(moved[0])))
}

fn expect_done(on: Handle, request: Request) -> Result<(), String> {
    match ask(on, request)?.0 {
        Reply::Done => Ok(()),
        other => Err(unexpected(&other)),
    }
}

fn open(dir: Handle, path: &str, how: How, rights: Rights) -> Result<Handle, String> {
    let request = Request::Open {
        path: path.to_string(),
        rights: rights.bits(),
        how,
    };
    match ask(dir, request)? {
        (Reply::Opened(..), Some(handle)) => Ok(handle),
        (other, _) => Err(format!("{path}: {}", unexpected(&other))),
    }
}

/// Runs `f` on the folder at `path` beneath `home`: `home` itself for "", else
/// a handle opened for it and closed after.
fn in_dir<T>(
    home: Handle,
    path: &str,
    rights: Rights,
    f: impl FnOnce(Handle) -> Result<T, String>,
) -> Result<T, String> {
    match path.trim_matches('/') {
        "" => f(home),
        path => {
            let dir = open(home, path, How::Existing, rights)?;
            let result = f(dir);
            close(dir);
            result
        }
    }
}

fn close(handle: Handle) {
    let _ = sys::handle_close(handle);
}

fn list(dir: Handle) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut after = None;
    loop {
        let request = Request::List {
            after,
            max: fsproto::MAX_LIST,
        };
        match ask(dir, request)?.0 {
            Reply::Listed { names: page, more } => {
                names.extend(page.into_iter().map(|l| match l.kind {
                    fsproto::Kind::Dir => format!("{}/", l.name),
                    fsproto::Kind::File => l.name,
                }));
                if more.is_none() || names.len() >= LIST_MAX {
                    return Ok(names);
                }
                after = more;
            }
            other => return Err(unexpected(&other)),
        }
    }
}

fn read_all(file: Handle) -> Result<String, String> {
    let mut bytes = Vec::new();
    while bytes.len() < READ_MAX {
        let request = Request::Read {
            off: bytes.len() as u64,
            len: fsproto::MAX_DATA as u32,
        };
        match ask(file, request)?.0 {
            Reply::Data(data) if data.is_empty() => break,
            Reply::Data(data) => {
                let short = data.len() < fsproto::MAX_DATA;
                bytes.extend_from_slice(&data);
                if short {
                    break;
                }
            }
            other => return Err(unexpected(&other)),
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn write_all(file: Handle, bytes: &[u8]) -> Result<(), String> {
    for (i, chunk) in bytes.chunks(fsproto::MAX_DATA).enumerate() {
        let request = Request::Write {
            off: (i * fsproto::MAX_DATA) as u64,
            data: chunk.to_vec(),
        };
        expect_done(file, request)?;
    }
    Ok(())
}

fn unexpected(reply: &Reply) -> String {
    match reply {
        Reply::Failed(error) => describe(*error),
        other => format!("the fs service answered {other:?}"),
    }
}

fn describe(error: Error) -> String {
    format!("{error:?}")
}
