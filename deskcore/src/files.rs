//! The editor's file door between its `comphost` and the shell (P4): calls go one
//! way, so `desktop` holds a file in pieces between them, as it holds an app's
//! effects (`effects`). It keeps a transfer only while it runs. One request a
//! call, as text, a piece of a file after a first line:
//!
//! - from the editor, `files start <ticket> read <path>` or
//!   `files start <ticket> write <new|old> <user|-> <total> <path>` → `ok`, or
//!   `err busy ...` when [`MAX_TRANSFERS`] are running already, or why not;
//! - from the editor, `files data <ticket> <offset>\n<bytes>` → `ok`: the next
//!   piece of a write, at most [`CHUNK`] bytes, in order;
//! - from the editor, `files poll <ticket>` → `pending`, `ok` (a write is done;
//!   a read has its bytes: `ok <total>`) or `err <why>`; and
//!   `files get <ticket> <offset>` → `ok <total>\n<bytes>`, a read's piece. A
//!   write's transfer is gone once its `ok` is told, a read's with its last piece
//!   or any `err`;
//! - from the shell, `files take` → `none`, or `job <id> <app> read <path>` /
//!   `job <id> <app> write <new|old> <user|-> <total> <path>`: the oldest
//!   transfer that is complete;
//! - from the shell, `files fetch <id> <offset>` → `ok\n<bytes>` (a write's
//!   piece), `files fill <id> <offset> <total>\n<bytes>` → `ok` (a read's
//!   piece, in order) and `files done <id> ok` or `files done <id> err <why>` →
//!   `ok`.
//!
//! Which app a transfer is for is `desktop`'s own record of the caller (the
//! kernel's name for its process), never anything in the message, and only the
//! editor ([`DOOR_APP`]) may start one; at most one transfer an app, and
//! [`MAX_TRANSFERS`] in all. A transfer nothing has touched for
//! [`IDLE_MS`] is dropped, and an app that starts another drops its old one, so
//! a restarted `comphost` or a dead shell cannot hold the door.
//!
//! The pieces are the transport's choice and the app does not see them: the
//! editor asks for and is told a whole file ([`CHUNK`] is only how many bytes a
//! message holds).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The one app that may use the door: the plan line's name.
pub const DOOR_APP: &str = "editor";
/// The shell, by the kernel's name for it.
pub const SHELL: &str = "shell";
/// Bytes of a file one message holds.
pub const CHUNK: usize = 6 * 1024;
/// The most bytes of a file a transfer holds.
pub const MAX_BYTES: usize = 256 * 1024;
/// Transfers running system-wide; past that a start is refused as busy.
pub const MAX_TRANSFERS: usize = 2;
/// Milliseconds without a touch after which a transfer is dropped.
pub const IDLE_MS: u64 = 120_000;
/// Longest path a transfer names.
pub const MAX_PATH: usize = 255;
/// Where the door's files are: the person's own tree. Anything else (`/system/...`)
/// is refused here and again by the shell.
pub const HOME: &str = "/home/user/";

/// What a transfer does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Read,
    /// Made (it must not exist) when `create`, else replaced.
    Write {
        create: bool,
    },
}

/// A complete transfer, as the shell takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: u32,
    pub app: String,
    pub op: Op,
    /// Whether a person's key or click caused the request (the app's word,
    /// carried by its `comphost`); the shell passes a write on only if so.
    pub user: bool,
    pub path: String,
    /// A write's bytes, or what a read's answer is to be.
    pub total: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    /// A write's pieces are coming.
    Receiving,
    /// Complete, waiting for the shell.
    Waiting,
    /// The shell has it.
    Running,
    /// Answered; a read's bytes wait for the app.
    Answered(Result<(), String>),
}

struct Transfer {
    job: Job,
    ticket: u32,
    bytes: Vec<u8>,
    phase: Phase,
    touched_ms: u64,
}

/// What a request is answered with, and whether the shell should wake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub bytes: Vec<u8>,
    pub wake: bool,
}

fn text(text: impl AsRef<str>) -> Reply {
    Reply {
        bytes: text.as_ref().as_bytes().to_vec(),
        wake: false,
    }
}

fn err(why: impl AsRef<str>) -> Reply {
    text(format!("err {}", why.as_ref()))
}

/// Every running transfer.
pub struct Files {
    /// The apps that may start one (the kernel's names for them).
    apps: Vec<String>,
    transfers: Vec<Transfer>,
    next_id: u32,
}

impl Default for Files {
    /// Only the editor may use the door.
    fn default() -> Self {
        Self::with_apps(&[DOOR_APP])
    }
}

/// Why `path` is not one a transfer names: absolute, plain segments, no
/// whitespace, colon, backslash or control character, at most [`MAX_PATH`] bytes.
pub fn check_path(path: &str) -> Result<(), String> {
    let plain = path
        .strip_prefix('/')
        .ok_or_else(|| format!("{path:?} is not an absolute path"))?;
    if path.len() > MAX_PATH {
        return Err(format!("the path is over {MAX_PATH} bytes"));
    }
    if !path.starts_with(HOME) {
        return Err(format!("{path}: the file door reaches only {HOME}..."));
    }
    let bad = |c: char| c.is_whitespace() || c.is_control() || c == ':' || c == '\\';
    if plain.chars().any(bad) {
        return Err(format!(
            "{path}: spaces, colons, backslashes and control characters are not allowed"
        ));
    }
    if plain
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("{path}: empty, . and .. segments are not allowed"));
    }
    Ok(())
}

/// A job line as `files take` gives it (`job <id> <app> read <path>` or
/// `job <id> <app> write <new|old> <user|-> <total> <path>`); `None` for `none`
/// or anything else.
pub fn parse_job(line: &str) -> Option<Job> {
    let words: Vec<&str> = line.split(' ').collect();
    let (id, app, op, user, total, path) = match words.as_slice() {
        ["job", id, app, "read", path] => (id, app, Op::Read, false, 0, path),
        [
            "job",
            id,
            app,
            "write",
            how @ ("new" | "old"),
            user @ ("user" | "-"),
            total,
            path,
        ] => {
            let op = Op::Write {
                create: *how == "new",
            };
            (id, app, op, *user == "user", total.parse().ok()?, path)
        }
        _ => return None,
    };
    Some(Job {
        id: id.parse().ok()?,
        app: app.to_string(),
        op,
        user,
        path: path.to_string(),
        total,
    })
}

impl Files {
    /// A door for `apps` (more than the editor, one day).
    pub fn with_apps(apps: &[&str]) -> Self {
        Self {
            apps: apps.iter().map(|a| a.to_string()).collect(),
            transfers: Vec::new(),
            next_id: 0,
        }
    }

    /// How many transfers are running.
    pub fn running(&self) -> usize {
        self.transfers.len()
    }

    /// Drops what nothing has touched for [`IDLE_MS`].
    pub fn expire(&mut self, now_ms: u64) {
        self.transfers
            .retain(|t| now_ms.saturating_sub(t.touched_ms) < IDLE_MS);
    }

    /// Answers one request from `caller` (the kernel's name for it), or `None`
    /// when it is not a file request.
    pub fn request(&mut self, caller: &str, bytes: &[u8], now_ms: u64) -> Option<Reply> {
        let (head, body) = match bytes.iter().position(|&b| b == b'\n') {
            Some(at) => (&bytes[..at], &bytes[at + 1..]),
            None => (bytes, &bytes[..0]),
        };
        let head = core::str::from_utf8(head).ok()?;
        let rest = head.strip_prefix("files ")?;
        self.expire(now_ms);
        let words: Vec<&str> = rest.split(' ').collect();
        let reply = if caller == SHELL {
            self.shell_request(&words, body, now_ms)
        } else if self.apps.iter().any(|app| app == caller) {
            self.app_request(caller, &words, body, now_ms)
        } else {
            err("only the editor and the shell use the file door")
        };
        Some(reply)
    }

    fn find(&mut self, app: &str, ticket: u32) -> Result<&mut Transfer, Reply> {
        self.transfers
            .iter_mut()
            .find(|t| t.job.app == app && t.ticket == ticket)
            .ok_or_else(|| err("that transfer is gone"))
    }

    fn by_id(&mut self, id: u32) -> Result<&mut Transfer, Reply> {
        self.transfers
            .iter_mut()
            .find(|t| t.job.id == id)
            .ok_or_else(|| err("that transfer is gone"))
    }

    fn drop_ticket(&mut self, app: &str, ticket: u32) {
        self.transfers
            .retain(|t| !(t.job.app == app && t.ticket == ticket));
    }

    fn app_request(&mut self, app: &str, words: &[&str], body: &[u8], now_ms: u64) -> Reply {
        let number = |word: &str| word.parse::<usize>().map_err(|_| err("not a number"));
        let ticket = |word: &str| word.parse::<u32>().map_err(|_| err("not a ticket"));
        match words {
            ["start", t, "read", path] => match ticket(t) {
                Ok(t) => self.start(app, t, Op::Read, false, 0, path, now_ms),
                Err(e) => e,
            },
            [
                "start",
                t,
                "write",
                how @ ("new" | "old"),
                user @ ("user" | "-"),
                total,
                path,
            ] => match (ticket(t), number(total)) {
                (Ok(t), Ok(total)) => {
                    let op = Op::Write {
                        create: *how == "new",
                    };
                    self.start(app, t, op, *user == "user", total, path, now_ms)
                }
                (Err(e), _) | (_, Err(e)) => e,
            },
            ["data", t, offset] => match (ticket(t), number(offset)) {
                (Ok(t), Ok(offset)) => self.data(app, t, offset, body, now_ms),
                (Err(e), _) | (_, Err(e)) => e,
            },
            ["poll", t] => match ticket(t) {
                Ok(t) => self.poll(app, t, now_ms),
                Err(e) => e,
            },
            ["get", t, offset] => match (ticket(t), number(offset)) {
                (Ok(t), Ok(offset)) => self.get(app, t, offset, now_ms),
                (Err(e), _) | (_, Err(e)) => e,
            },
            _ => err("not a file request"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        app: &str,
        ticket: u32,
        op: Op,
        user: bool,
        total: usize,
        path: &str,
        now_ms: u64,
    ) -> Reply {
        if let Err(why) = check_path(path) {
            return err(why);
        }
        if total > MAX_BYTES {
            return err(format!("a file is at most {MAX_BYTES} bytes"));
        }
        // An app has one transfer at a time: a new start drops its old one.
        self.transfers.retain(|t| t.job.app != app);
        if self.transfers.len() >= MAX_TRANSFERS {
            return err(format!(
                "busy: {MAX_TRANSFERS} files are on their way; try again"
            ));
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let phase = match op {
            Op::Read => Phase::Waiting,
            Op::Write { .. } if total == 0 => Phase::Waiting,
            Op::Write { .. } => Phase::Receiving,
        };
        let wake = phase == Phase::Waiting;
        self.transfers.push(Transfer {
            job: Job {
                id,
                app: app.to_string(),
                op,
                user,
                path: path.to_string(),
                total,
            },
            ticket,
            bytes: Vec::new(),
            phase,
            touched_ms: now_ms,
        });
        Reply {
            bytes: b"ok".to_vec(),
            wake,
        }
    }

    fn data(&mut self, app: &str, ticket: u32, offset: usize, body: &[u8], now_ms: u64) -> Reply {
        let transfer = match self.find(app, ticket) {
            Ok(t) => t,
            Err(e) => return e,
        };
        let bad = body.len() > CHUNK
            || transfer.phase != Phase::Receiving
            || offset != transfer.bytes.len()
            || transfer.bytes.len() + body.len() > transfer.job.total;
        if bad {
            self.drop_ticket(app, ticket);
            return err("a piece out of order or over what was said");
        }
        transfer.bytes.extend_from_slice(body);
        transfer.touched_ms = now_ms;
        let wake = transfer.bytes.len() == transfer.job.total;
        if wake {
            transfer.phase = Phase::Waiting;
        }
        Reply {
            bytes: b"ok".to_vec(),
            wake,
        }
    }

    fn poll(&mut self, app: &str, ticket: u32, now_ms: u64) -> Reply {
        let transfer = match self.find(app, ticket) {
            Ok(t) => t,
            Err(e) => return e,
        };
        transfer.touched_ms = now_ms;
        match transfer.phase.clone() {
            Phase::Answered(Err(why)) => {
                self.drop_ticket(app, ticket);
                err(why)
            }
            Phase::Answered(Ok(())) if transfer.job.op == Op::Read => {
                text(format!("ok {}", transfer.bytes.len()))
            }
            Phase::Answered(Ok(())) => {
                self.drop_ticket(app, ticket);
                text("ok")
            }
            _ => text("pending"),
        }
    }

    fn get(&mut self, app: &str, ticket: u32, offset: usize, now_ms: u64) -> Reply {
        let transfer = match self.find(app, ticket) {
            Ok(t) => t,
            Err(e) => return e,
        };
        if transfer.phase != Phase::Answered(Ok(())) || transfer.job.op != Op::Read {
            return err("there is nothing to get");
        }
        let total = transfer.bytes.len();
        if offset > total {
            self.drop_ticket(app, ticket);
            return err("past the end of the file");
        }
        let end = total.min(offset + CHUNK);
        transfer.touched_ms = now_ms;
        let mut out = format!("ok {total}\n").into_bytes();
        out.extend_from_slice(&transfer.bytes[offset..end]);
        if end == total {
            self.drop_ticket(app, ticket);
        }
        Reply {
            bytes: out,
            wake: false,
        }
    }

    fn shell_request(&mut self, words: &[&str], body: &[u8], now_ms: u64) -> Reply {
        let id = |word: &str| word.parse::<u32>().map_err(|_| err("not an id"));
        let number = |word: &str| word.parse::<usize>().map_err(|_| err("not a number"));
        match words {
            ["take"] => self.take(now_ms),
            ["fetch", i, offset] => match (id(i), number(offset)) {
                (Ok(i), Ok(offset)) => self.fetch(i, offset, now_ms),
                (Err(e), _) | (_, Err(e)) => e,
            },
            ["fill", i, offset, total] => match (id(i), number(offset), number(total)) {
                (Ok(i), Ok(offset), Ok(total)) => self.fill(i, offset, total, body, now_ms),
                (Err(e), ..) | (_, Err(e), _) | (.., Err(e)) => e,
            },
            ["done", i, "ok"] => match id(i) {
                Ok(i) => self.done(i, Ok(()), now_ms),
                Err(e) => e,
            },
            ["done", i, "err", why @ ..] => match id(i) {
                Ok(i) => self.done(i, Err(why.join(" ")), now_ms),
                Err(e) => e,
            },
            _ => err("not a file request"),
        }
    }

    fn take(&mut self, now_ms: u64) -> Reply {
        let Some(transfer) = self
            .transfers
            .iter_mut()
            .find(|t| t.phase == Phase::Waiting)
        else {
            return text("none");
        };
        transfer.phase = Phase::Running;
        transfer.touched_ms = now_ms;
        let job = &transfer.job;
        let line = match job.op {
            Op::Read => format!("job {} {} read {}", job.id, job.app, job.path),
            Op::Write { create } => format!(
                "job {} {} write {} {} {} {}",
                job.id,
                job.app,
                if create { "new" } else { "old" },
                if job.user { "user" } else { "-" },
                job.total,
                job.path
            ),
        };
        text(line)
    }

    fn fetch(&mut self, id: u32, offset: usize, now_ms: u64) -> Reply {
        let transfer = match self.by_id(id) {
            Ok(t) => t,
            Err(e) => return e,
        };
        if transfer.phase != Phase::Running || transfer.job.op == Op::Read {
            return err("there is nothing to fetch");
        }
        let total = transfer.bytes.len();
        if offset > total {
            return err("past the end of the file");
        }
        transfer.touched_ms = now_ms;
        let end = total.min(offset + CHUNK);
        let mut out = b"ok\n".to_vec();
        out.extend_from_slice(&transfer.bytes[offset..end]);
        Reply {
            bytes: out,
            wake: false,
        }
    }

    fn fill(&mut self, id: u32, offset: usize, total: usize, body: &[u8], now_ms: u64) -> Reply {
        let transfer = match self.by_id(id) {
            Ok(t) => t,
            Err(e) => return e,
        };
        let bad = transfer.phase != Phase::Running
            || transfer.job.op != Op::Read
            || body.len() > CHUNK
            || total > MAX_BYTES
            || offset != transfer.bytes.len()
            || offset + body.len() > total
            || (offset > 0 && total != transfer.job.total);
        if bad {
            return err("a piece out of order or over what was said");
        }
        transfer.job.total = total;
        transfer.bytes.extend_from_slice(body);
        transfer.touched_ms = now_ms;
        text("ok")
    }

    fn done(&mut self, id: u32, outcome: Result<(), String>, now_ms: u64) -> Reply {
        let transfer = match self.by_id(id) {
            Ok(t) => t,
            Err(e) => return e,
        };
        if transfer.phase != Phase::Running {
            return err("that transfer is not running");
        }
        let short = transfer.job.op == Op::Read
            && outcome.is_ok()
            && transfer.bytes.len() != transfer.job.total;
        transfer.phase = if short {
            Phase::Answered(Err(String::from("the file was not all there")))
        } else {
            Phase::Answered(outcome)
        };
        // A write's bytes are not needed any more.
        if transfer.job.op != Op::Read {
            transfer.bytes = Vec::new();
        }
        transfer.touched_ms = now_ms;
        text("ok")
    }
}
