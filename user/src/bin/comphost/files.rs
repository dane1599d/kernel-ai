//! The editor app's way to the person's files (P4): what `kai:files/files`
//! queued in the shown version goes to `desktop` in pieces (`deskcore::files`),
//! which holds the file while the shell does the read or the write with its own
//! `fs_*` rows, so tiers, the log and undo stay the OS's. The app is told a whole
//! file, or why not, through `replies.on-done`; it does not see the pieces, and a
//! shared page could replace them without it changing.
//!
//! The relay never waits for the shell: it makes the call that moves a transfer
//! one step, and goes on being polled by comphost's loop (20 ms while the window
//! is open) until the shell has answered. One transfer runs at a time; a request
//! sent while one runs waits its turn. A standby version, rehearsed beside the
//! active one during probation, has its queue emptied and dropped.

use crate::started::shown_serial;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use deskcore::files::{CHUNK, IDLE_MS, MAX_BYTES};
use kabi::handle::Handle;
use livecore::host::{Env, Host};
use user::{println, sys};
use wasmhost::{FileOp, FilesRequest, LiveInstance};

/// Most steps one pump makes, so a big file cannot keep comphost from serving.
const STEPS: usize = 16;
/// Most requests waiting their turn; past that a new one is answered busy.
const MAX_WAITING: usize = 4;
/// Milliseconds between two polls for the shell's answer.
const POLL_EVERY_MS: u64 = 40;
/// How long one call to `desktop` may take.
const CALL_MS: u64 = 5_000;
/// Why a request is answered with an error when there is no desktop to go to.
const NO_DESKTOP: &str = "there is no desktop to move the file through";

/// Where a transfer is.
enum Step {
    /// Not started: `desktop` has not heard of it.
    Start,
    /// A write's pieces going; the next starts at `offset`.
    Send { offset: usize },
    /// Everything is sent, or a read is asked for: waiting for the shell.
    Poll { last_ms: u64 },
    /// A read's pieces coming: what has come.
    Get { got: Vec<u8> },
}

/// A request and the instance that asked it: tickets restart at 0 in a new instance, so
/// an answer is delivered only while that instance is the one shown.
struct Queued {
    serial: u64,
    request: FilesRequest,
}

struct Active {
    serial: u64,
    request: FilesRequest,
    step: Step,
    began_ms: u64,
}

impl Active {
    fn new(queued: Queued) -> Self {
        Self {
            serial: queued.serial,
            request: queued.request,
            step: Step::Start,
            began_ms: now_ms(),
        }
    }
}

/// What one step came to.
enum Next {
    /// Make the next step now.
    Again(Active),
    /// Nothing more to do until the shell answers.
    Wait(Active),
    /// Tell the app.
    Done(Active, Result<Vec<u8>, String>),
}

/// The editor's file requests on their way, and the route to `desktop`.
pub struct Relay {
    route: Option<Handle>,
    waiting: VecDeque<Queued>,
    active: Option<Active>,
    reply: Vec<u8>,
}

fn now_ms() -> u64 {
    sys::time_ns() / 1_000_000
}

fn verb(op: &FileOp) -> (&'static str, &str) {
    match op {
        FileOp::Read { path } => ("read", path),
        FileOp::Write { path, .. } => ("write", path),
    }
}

impl Relay {
    /// A relay over the plan line's first route, `desktop`'s, if it has one.
    pub fn new() -> Self {
        Self {
            route: user::route(0),
            waiting: VecDeque::new(),
            active: None,
            reply: alloc::vec![0u8; kabi::MAX_MESSAGE],
        }
    }

    /// Whether a transfer is on its way, which keeps comphost polling.
    pub fn busy(&self) -> bool {
        self.active.is_some() || !self.waiting.is_empty()
    }

    /// Takes what the shown version queued, moves the transfer a few steps and
    /// tells the app what came of the ones that are done.
    pub fn pump<E: Env<Instance = LiveInstance>>(&mut self, host: &mut Host<E>) {
        // After a swap the old version's work is dropped, not answered to the new one: its
        // tickets mean nothing there. `desktop` drops the old transfer when the new version
        // starts its own, or when it ages out.
        let shown = shown_serial(host);
        self.waiting.retain(|queued| Some(queued.serial) == shown);
        if self
            .active
            .as_ref()
            .is_some_and(|a| Some(a.serial) != shown)
        {
            println!(
                "comphost: files: the version that asked was swapped out; its transfer is dropped"
            );
            self.active = None;
        }
        for (ticket, why) in self.take(host) {
            if !answer(host, ticket, Err(why)) {
                return;
            }
        }
        for _ in 0..STEPS {
            let Some(active) = self
                .active
                .take()
                .or_else(|| self.waiting.pop_front().map(Active::new))
            else {
                return;
            };
            match self.advance(active) {
                Next::Again(active) => self.active = Some(active),
                Next::Wait(active) => {
                    self.active = Some(active);
                    return;
                }
                Next::Done(active, outcome) => {
                    let (what, path) = verb(&active.request.op);
                    println!(
                        "comphost: files: {what} {path} (person {}) {} in {} ms",
                        active.request.user,
                        match &outcome {
                            Ok(bytes) => format!("answered {} bytes", bytes.len()),
                            Err(why) => format!("failed: {why}"),
                        },
                        now_ms().saturating_sub(active.began_ms),
                    );
                    if !answer(host, active.request.ticket, outcome) {
                        // The app was reloaded: what it asked is not its to be told.
                        self.waiting.clear();
                        return;
                    }
                }
            }
        }
    }

    /// Takes the shown version's queue; the tickets of those there is no room
    /// for, with why.
    fn take<E: Env<Instance = LiveInstance>>(&mut self, host: &mut Host<E>) -> Vec<(u32, String)> {
        let mut queued: Vec<FilesRequest> = Vec::new();
        let mut serial = 0;
        host.each_instance(|instance, _, shown| {
            if let LiveInstance::V2(core) = instance {
                let taken = core.take_files();
                if shown {
                    queued = taken;
                    serial = core.serial();
                }
            }
        });
        let mut refused = Vec::new();
        for request in queued {
            if self.waiting.len() >= MAX_WAITING {
                let why = String::from("too many files are waiting; try again");
                refused.push((request.ticket, why));
            } else {
                self.waiting.push_back(Queued { serial, request });
            }
        }
        refused
    }

    /// One call to `desktop`: its reply, or why there is none.
    fn call(&mut self, message: &[u8]) -> Result<Vec<u8>, String> {
        let route = self.route.ok_or_else(|| String::from(NO_DESKTOP))?;
        sys::call_route(route, message, &mut self.reply, CALL_MS)
            .map(|len| self.reply[..len.min(self.reply.len())].to_vec())
            .map_err(|error| format!("desktop did not answer ({error:?})"))
    }

    /// One call to `desktop` that must answer `ok`; why if it did not.
    fn call_ok(&mut self, message: &[u8]) -> Result<(), String> {
        let reply = self.call(message)?;
        match reply.as_slice() {
            b"ok" => Ok(()),
            other => Err(reason(other)),
        }
    }

    /// Moves a transfer one step.
    fn advance(&mut self, mut active: Active) -> Next {
        let ticket = active.request.ticket;
        match core::mem::replace(&mut active.step, Step::Start) {
            Step::Start => {
                let message = match &active.request.op {
                    FileOp::Read { path } => format!("files start {ticket} read {path}"),
                    FileOp::Write { .. } if !active.request.user => {
                        let why = "a save needs a key or click of the person";
                        return Next::Done(active, Err(String::from(why)));
                    }
                    FileOp::Write { path, data, create } => format!(
                        "files start {ticket} write {} user {} {path}",
                        if *create { "new" } else { "old" },
                        data.len()
                    ),
                };
                if let Err(why) = self.call_ok(message.as_bytes()) {
                    return Next::Done(active, Err(why));
                }
                active.step = match &active.request.op {
                    FileOp::Write { data, .. } if !data.is_empty() => Step::Send { offset: 0 },
                    _ => Step::Poll { last_ms: 0 },
                };
                Next::Again(active)
            }
            Step::Send { offset } => {
                let FileOp::Write { data, .. } = &active.request.op else {
                    return Next::Done(active, Err(String::from("not a write")));
                };
                let end = data.len().min(offset + CHUNK);
                let mut message = format!("files data {ticket} {offset}\n").into_bytes();
                message.extend_from_slice(&data[offset..end]);
                let sent = self.call_ok(&message);
                let last = end == data.len();
                active.step = if last {
                    Step::Poll { last_ms: 0 }
                } else {
                    Step::Send { offset: end }
                };
                match sent {
                    Ok(()) => Next::Again(active),
                    Err(why) => Next::Done(active, Err(why)),
                }
            }
            Step::Poll { last_ms } => {
                let now = now_ms();
                if now.saturating_sub(last_ms) < POLL_EVERY_MS {
                    active.step = Step::Poll { last_ms };
                    return Next::Wait(active);
                }
                if now.saturating_sub(active.began_ms) > IDLE_MS {
                    return Next::Done(active, Err(String::from("the shell did not answer")));
                }
                let reply = match self.call(format!("files poll {ticket}").as_bytes()) {
                    Ok(reply) => reply,
                    Err(why) => return Next::Done(active, Err(why)),
                };
                let reply = String::from_utf8_lossy(&reply).into_owned();
                let is_read = matches!(active.request.op, FileOp::Read { .. });
                match reply.as_str() {
                    "pending" => {
                        active.step = Step::Poll { last_ms: now };
                        Next::Wait(active)
                    }
                    "ok" if !is_read => Next::Done(active, Ok(Vec::new())),
                    _ if is_read && reply.starts_with("ok ") => {
                        active.step = Step::Get { got: Vec::new() };
                        Next::Again(active)
                    }
                    other => Next::Done(active, Err(reason(other.as_bytes()))),
                }
            }
            Step::Get { mut got } => {
                let reply = match self.call(format!("files get {ticket} {}", got.len()).as_bytes())
                {
                    Ok(reply) => reply,
                    Err(why) => return Next::Done(active, Err(why)),
                };
                let Some((total, piece)) = split_piece(&reply) else {
                    return Next::Done(active, Err(reason(&reply)));
                };
                got.extend_from_slice(piece);
                if got.len() > total || (got.len() < total && piece.is_empty()) {
                    return Next::Done(
                        active,
                        Err(String::from("the file changed while it was read")),
                    );
                }
                if got.len() == total {
                    return Next::Done(active, Ok(got));
                }
                active.step = Step::Get { got };
                Next::Again(active)
            }
        }
    }
}

/// `ok <total>\n<bytes>` as the total and the bytes.
fn split_piece(reply: &[u8]) -> Option<(usize, &[u8])> {
    let at = reply.iter().position(|&b| b == b'\n')?;
    let total: usize = core::str::from_utf8(&reply[..at])
        .ok()?
        .strip_prefix("ok ")?
        .parse()
        .ok()?;
    // A file is at most what the door holds; a bigger total is not desktop's.
    (total <= MAX_BYTES).then_some((total, &reply[at + 1..]))
}

/// What an `err <why>` reply says; anything else is an odd answer.
fn reason(reply: &[u8]) -> String {
    let text = String::from_utf8_lossy(reply);
    match text.strip_prefix("err ") {
        Some(why) => String::from(why),
        None => String::from("desktop gave an answer comphost does not know"),
    }
}

/// Tells the app the answer: a call into it like a window's input, a transaction
/// of its own, so a fault in `on-done` is the version's, reported and reloaded.
/// False when it faulted or nothing serves.
fn answer<E: Env<Instance = LiveInstance>>(
    host: &mut Host<E>,
    ticket: u32,
    outcome: Result<Vec<u8>, String>,
) -> bool {
    if crate::ui::serve_shown(host, |core| core.files_reply(ticket, outcome)).is_none() {
        println!("comphost: files: the answer for ticket {ticket} did not reach the app");
        return false;
    }
    true
}
