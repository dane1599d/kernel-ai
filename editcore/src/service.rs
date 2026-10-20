//! The ring-3 `edit` service over an environment trait: it opens a session on a
//! version read from the kernel's store, runs the editor's commands, builds on the
//! host and uploads what comes back, and relays the kernel's git syncs. It answers
//! only the kernel; the kernel shows the change and asks before anything runs.

use crate::command::{self, Command, USAGE};
use crate::ipc::{self, At, Ipc};
use crate::session::Session;
use crate::wire::{self, Synced};
use crate::{MAX_PRINTED, editable};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::Info;
use livecore::{artifact, diff};
use llmproto::http;

mod building;
mod replay;

pub(crate) use building::{MOVED, location};

/// How long a build may take, inside the kernel's wait for this service.
pub const BUILD_TIMEOUT_MS: u64 = 150_000;
/// How long a sync may take.
pub const SYNC_TIMEOUT_MS: u64 = 30_000;
/// Lines `p` prints without a range.
const PAGE: usize = 10;

/// What the service needs from the kernel.
pub trait Env {
    fn info(&mut self, component: &str) -> Result<Info, Error>;
    /// A version's artifact from the store.
    fn artifact(&mut self, component: &str, label: &str) -> Result<Vec<u8>, Error>;
    /// Uploads an artifact under the label the store assigns next.
    fn put(
        &mut self,
        component: &str,
        artifact: &[u8],
        base: &str,
        commit: &str,
    ) -> Result<(), Error>;
    /// Sends an HTTP request to the build service; the response.
    fn build(&mut self, request: &[u8], timeout_ms: u64) -> Result<Vec<u8>, Error>;
    /// Has the kernel send `ctl <request>` to a component; only a request that
    /// changes nothing goes.
    fn control(&mut self, component: &str, request: &str) -> Result<Vec<u8>, Error>;
    fn log(&mut self, text: &str);
}

pub struct Service {
    /// The build service, as `IP:PORT` for the Host header.
    host: String,
    token: String,
    pub session: Option<Session>,
}

fn ok(text: impl AsRef<str>) -> String {
    format!("ok {}", text.as_ref())
}

fn err(text: impl AsRef<str>) -> String {
    format!("err {}", text.as_ref())
}

/// Cuts a reply to what one IPC reply holds.
fn fit(mut reply: String) -> String {
    let max = kabi::MAX_MESSAGE - 8;
    if reply.len() > max {
        reply.truncate(reply.floor_char_boundary(max));
        reply.push_str("\n...");
    }
    reply
}

impl Service {
    pub fn new(host: &str, token: &str) -> Self {
        Self {
            host: host.to_string(),
            token: token.to_string(),
            session: None,
        }
    }

    /// Answers one request from `caller` (0 is the kernel).
    pub fn handle(&mut self, env: &mut dyn Env, caller: u64, request: &[u8]) -> String {
        if caller != 0 {
            return err("the edit service answers only the kernel");
        }
        let reply = match ipc::decode_ipc(request) {
            Ok(Ipc::Open {
                component,
                label,
                at,
            }) => self.open(env, component, label, at),
            Ok(Ipc::Cmd(line)) => self.command(env, line),
            Ok(Ipc::Sync { seq, job }) => self.sync(env, seq, &job),
            Ok(Ipc::Replay { from, entries }) => self.replay(env, from, entries),
            Err(why) => err(why),
        };
        fit(reply)
    }

    /// Opens or resumes a session; with `at`, the cursor goes there, and edits
    /// that are not built are never dropped for it.
    fn open(
        &mut self,
        env: &mut dyn Env,
        component: &str,
        label: Option<&str>,
        at: Option<At<'_>>,
    ) -> String {
        let Some(editable) = editable(component) else {
            return err(format!("{component} cannot be edited; notes can"));
        };
        let info = match env.info(component) {
            Ok(info) => info,
            Err(error) => return err(format!("no component {component} ({error:?})")),
        };
        let label = match label {
            Some(label) => label.to_string(),
            None if info.active().is_empty() => {
                return err(format!("{component} has not started yet"));
            }
            None => info.active().to_string(),
        };
        let mut dropped = String::new();
        // The session whose unbuilt edits an open-at keeps, if the version opens.
        let mut kept = None;
        if let Some(session) = self.session.as_mut() {
            if session.component == component && session.label == label {
                if let Some(at) = at {
                    return goto_again(session, at);
                }
                let state = if session.changed() {
                    ", with your edits"
                } else {
                    ""
                };
                return ok(format!("editing {component} {label} again{state}"));
            }
            if session.changed() && session.built.is_none() {
                if at.is_some() {
                    kept = Some(format!("{} {}", session.component, session.label));
                } else {
                    dropped = format!("; dropped the unbuilt edits to {}", session.label);
                }
            }
        }
        let bytes = match env.artifact(component, &label) {
            Ok(bytes) => bytes,
            Err(Error::NotFound) => return err(format!("no version {component} {label}")),
            Err(error) => return err(format!("cannot read {component} {label}: {error:?}")),
        };
        let parsed = match artifact::parse(&bytes) {
            Ok(parsed) => parsed,
            Err(why) => return err(format!("{component} {label}: {why}")),
        };
        let files = artifact::source_files(parsed.source).unwrap_or_default();
        let inside = format!("{}/", editable.dir);
        let manifest = format!("{}/Cargo.toml", editable.dir);
        let own = files.iter().all(|(path, _)| path.starts_with(&inside))
            && files.iter().any(|(path, _)| *path == manifest);
        if !own {
            return err(format!(
                "{component} {label} is not editable: its source is not {}",
                editable.dir
            ));
        }
        let mut session = match Session::open(component, &label, parsed.source) {
            Ok(session) => session,
            Err(why) => return err(why),
        };
        if let Some(kept) = kept {
            return err(format!(
                "{kept} has edits that are not built; build them or reset before opening {component} {label} there"
            ));
        }
        let files = session.files().len();
        let reply = match at.map(|at| goto(&mut session, at)) {
            Some(Ok(place)) => ok(format!(
                "editing {component} {label} at {place}: {files} files{dropped}"
            )),
            Some(Err(why)) => ok(format!(
                "editing {component} {label}: {files} files; {why}{dropped}"
            )),
            None => ok(format!(
                "editing {component} {label}: {files} files; {}, {} lines{dropped}",
                session.current().path,
                session.current().len()
            )),
        };
        self.session = Some(session);
        reply
    }

    fn command(&mut self, env: &mut dyn Env, line: &str) -> String {
        let command = match command::parse(line) {
            Ok(command) => command,
            Err(why) => return err(why),
        };
        if command == Command::Help {
            return ok(format!("commands:\n{USAGE}"));
        }
        let Some(session) = self.session.as_mut() else {
            return err("no file is open; :edit <component> first");
        };
        match command {
            Command::Build => self.build(env),
            Command::Try => rehearse(env, session),
            command => run(session, command),
        }
    }

    fn sync(&mut self, env: &mut dyn Env, seq: u64, job: &wire::SyncRequest<'_>) -> String {
        let body = wire::encode_sync(job);
        let body = match self.post(env, "/sync", &body, SYNC_TIMEOUT_MS) {
            Ok(body) => body,
            Err(why) => return err(format!("build service: {why}")),
        };
        match wire::decode_synced(&body) {
            Ok(Synced::Moved { sha, .. } | Synced::Recorded { sha, .. }) => {
                ok(format!("synced {seq} {sha}"))
            }
            Ok(Synced::Untracked) => ok(format!("untracked {seq}")),
            Err(why) => err(format!("build service: a malformed reply ({why})")),
        }
    }

    /// POSTs to the build service; the body of a 200 response.
    fn post(
        &self,
        env: &mut dyn Env,
        path: &str,
        body: &[u8],
        timeout_ms: u64,
    ) -> Result<Vec<u8>, String> {
        let token = [("X-Kai-Token", self.token.as_str())];
        let request = http::post(&self.host, path, "application/octet-stream", &token, body);
        let response = env
            .build(&request, timeout_ms)
            .map_err(|error| format!("{error:?}"))?;
        let response =
            http::parse_response(&response, true).map_err(|error| format!("{error:?}"))?;
        if response.status != 200 {
            let body = String::from_utf8_lossy(&response.body);
            let first = body.lines().next().unwrap_or("");
            return Err(format!("HTTP {} {}", response.status, first));
        }
        Ok(response.body)
    }
}

/// Moves the cursor to `at`: where it is now, or why it cannot go there.
fn goto(session: &mut Session, at: At<'_>) -> Result<String, String> {
    if session.goto(at.path, at.line as usize) {
        Ok(format!("{}:{}", at.path, session.line()))
    } else {
        Err(format!(
            "{} is not in {} {}",
            at.path, session.component, session.label
        ))
    }
}

/// Moves the open session's cursor to `at`. A line in a file the founder changed
/// is the stored version's, so the reply says it may have moved.
fn goto_again(session: &mut Session, at: At<'_>) -> String {
    let edited = session
        .files()
        .iter()
        .position(|file| file.path == at.path)
        .is_some_and(|index| session.file_changed(index));
    let place = match goto(session, at) {
        Ok(place) => place,
        Err(why) => return err(why),
    };
    let (component, label) = (&session.component, &session.label);
    if edited {
        ok(format!(
            "editing {component} {label} at {place}, with your edits; the line is {label}'s as stored, and your edits may have moved it"
        ))
    } else {
        ok(format!("editing {component} {label} at {place}"))
    }
}

fn numbered(session: &Session, from: usize, to: usize) -> String {
    let file = session.current();
    let mut out = format!("{} lines {from} to {to} of {}", file.path, file.len());
    for n in from..=to {
        if let Some(text) = file.line(n) {
            out.push_str(&format!("\n{n:>4} | {text}"));
        }
    }
    out
}

fn at(session: &Session, n: usize) -> String {
    let file = session.current();
    format!("{}:{n}: {}", file.path, file.line(n).unwrap_or(""))
}

/// Every command but build, on the session.
fn run(session: &mut Session, command: Command<'_>) -> String {
    let result = match command {
        Command::Files => {
            let current = session.current().path.clone();
            let mut out = format!("{} files; * changed, > current", session.files().len());
            for (index, file) in session.files().iter().enumerate() {
                let mark = match (session.file_changed(index), file.path == current) {
                    (true, true) => "*>",
                    (true, false) => "* ",
                    (false, true) => " >",
                    (false, false) => "  ",
                };
                out.push_str(&format!("\n{mark} {} ({} lines)", file.path, file.len()));
            }
            Ok(out)
        }
        Command::Open(name) => session.select(name).map(|()| {
            format!(
                "{}, {} lines",
                session.current().path,
                session.current().len()
            )
        }),
        Command::Print { from, to } => {
            let len = session.current().len();
            let from = from.unwrap_or(session.line().max(1));
            let to = to
                .unwrap_or(from.saturating_add(PAGE - 1))
                .min(len)
                .min(from.saturating_add(MAX_PRINTED - 1));
            if len == 0 || from > len {
                Err(format!("{} has {len} lines", session.current().path))
            } else {
                let text = numbered(session, from, to);
                let path = session.current().path.clone();
                session.goto(&path, to + 1);
                Ok(text)
            }
        }
        Command::Find(text) => {
            let found = session.find(text);
            match found.first() {
                None => Err(format!("no line holds {text:?}")),
                Some(&(file, line)) => {
                    let mut out = format!("{} lines hold {text:?}", found.len());
                    for &(index, n) in found.iter().take(MAX_PRINTED) {
                        let file = &session.files()[index];
                        out.push_str(&format!(
                            "\n{}:{n}: {}",
                            file.path,
                            file.line(n).unwrap_or("")
                        ));
                    }
                    let path = session.files()[file].path.clone();
                    session.goto(&path, line);
                    Ok(out)
                }
            }
        }
        Command::Substitute { line, old, new } => {
            session.substitute(line, old, new).map(|n| at(session, n))
        }
        Command::Change(n, text) => session.change(n, text).map(|()| at(session, n)),
        Command::Insert(n, text) => session.insert(n, text).map(|()| at(session, n)),
        Command::Append(n, text) => session.insert(n + 1, text).map(|()| at(session, n + 1)),
        Command::Delete(from, to) => session.delete(from, to).map(|()| {
            format!(
                "deleted {} lines; {} has {}",
                to - from + 1,
                session.current().path,
                session.current().len()
            )
        }),
        Command::Undo => match session.undo() {
            true => Ok(format!("undone; {}", at(session, session.line()))),
            false => Err("nothing to undo".into()),
        },
        Command::Diff => {
            let bundle = session.to_bundle();
            match diff::changes(session.original(), &bundle) {
                Ok(changes) if changes.is_empty() => Ok(format!(
                    "no changes since {} {}",
                    session.component, session.label
                )),
                Ok(changes) => {
                    let lines = diff::render(&changes);
                    let mut out = format!("changes since {} {}", session.component, session.label);
                    for line in lines.iter().take(MAX_PRINTED) {
                        out.push('\n');
                        out.push_str(line);
                    }
                    if lines.len() > MAX_PRINTED {
                        out.push_str(&format!("\n... {} more lines", lines.len() - MAX_PRINTED));
                    }
                    Ok(out)
                }
                Err(why) => Err(why.to_string()),
            }
        }
        Command::Reset => {
            session.reset();
            Ok(format!(
                "back to {} {} as opened",
                session.component, session.label
            ))
        }
        Command::Build | Command::Try | Command::Help => unreachable!("handled before"),
    };
    match result {
        Ok(text) => ok(text),
        Err(why) => err(why),
    }
}

/// `try`: the kernel sends `ctl try <built>` to the component, which rehearses the
/// swap on copies of its state; its answer is passed on.
fn rehearse(env: &mut dyn Env, session: &Session) -> String {
    let component = &session.component;
    let Some(built) = session.built.as_deref() else {
        return err("nothing is built from this text; build first, then try");
    };
    match env.control(component, &format!("try {built}")) {
        Ok(reply) => {
            let reply = String::from_utf8_lossy(&reply).into_owned();
            if reply.starts_with("ok ") || reply.starts_with("err ") {
                reply
            } else {
                err(format!("{component} answered {reply:?}"))
            }
        }
        Err(error) => err(format!("cannot reach {component}: {error:?}")),
    }
}
