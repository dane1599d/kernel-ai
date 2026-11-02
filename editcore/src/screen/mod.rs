//! The full-screen editor's model (M9d steps 10-12): what a screen of the
//! editor holds and does, without a screen, a clock or a system call.
//!
//! The edit service stays the one place a session lives, so the kernel's ed-style
//! editor, a restarted service's journal and this editor all see the same text.
//! A [`Screen`] is its client. It is sans-IO: keys and clicks go in through
//! [`Screen::input`]; requests for the service come out of
//! [`Screen::next_request`], one at a time, and the answers go back through
//! [`Screen::reply`]. The transport is the caller's: a ring-3 program calls the
//! service at once, an app sends an effect and waits for its result. What the
//! founder types stays in a local draft line and is sent as one `c` when the
//! cursor leaves the line; Enter and joining lines are the service's atomic
//! `split` and `join`, so one `undo` takes each back.
//!
//! [`Screen::view`] says what to draw (rows, a status line, the cursor), and
//! [`Screen::draft`] what to keep so a crash loses nothing ([`draft`]).

mod answer;
pub mod draft;
mod edit;
pub mod input;
mod keys;
pub mod link;
pub mod pager;
pub(crate) mod render;
pub mod reply;

pub use draft::Draft;
pub use input::{Input, Key};
pub use render::{Row, View};

use crate::ipc::{self, Ipc};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use pager::Pager;

/// Most lines a screen asks for at a time.
pub const WINDOW: usize = crate::service::view::MAX_VIEW;
/// How often a running build is looked at, in milliseconds.
pub const POLL_MS: u64 = 500;

/// What a screen asks the edit service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open (or resume) a version, at a line if one is given.
    Open {
        component: String,
        label: Option<String>,
        at: Option<(String, u32)>,
    },
    /// One command line, as typed in the kernel's edit mode.
    Cmd(String),
}

impl Request {
    /// The message for the edit service (`ipc::encode_ipc`).
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Open {
                component,
                label,
                at,
            } => ipc::encode_ipc(&Ipc::Open {
                component,
                label: label.as_deref(),
                at: at
                    .as_ref()
                    .map(|(path, line)| ipc::At { path, line: *line }),
            }),
            Self::Cmd(line) => ipc::encode_ipc(&Ipc::Cmd(line)),
        }
    }
}

/// A request and the number its answer comes back under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub id: u32,
    pub request: Request,
}

/// What a request was for, which decides what its answer does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Open,
    /// `view 1,1`: learn the file, its length and the service's cursor.
    Meta,
    /// Lines `from` to `to`.
    Window(usize, usize),
    Files,
    /// An edit; its command is kept in the draft once the service says `ok`.
    Edit(String),
    /// An edit that moves things the screen cannot predict (`undo`, `reset`).
    Reload(String),
    /// Replaying a draft's command.
    Replay(String),
    Build,
    Status,
    /// A reply to show in a pager.
    Show(String),
    /// Switch to another file, then go to a line.
    OpenFile(usize),
}

#[derive(Clone, Debug)]
pub(crate) struct Pending {
    pub kind: Kind,
    pub request: Request,
    /// The screen's [`Screen::epoch`] when it was queued.
    pub epoch: u32,
}

/// A line the screen holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub text: String,
    pub exact: bool,
}

/// The file on screen, as far as it is loaded.
#[derive(Clone, Debug, Default)]
pub(crate) struct Doc {
    pub path: String,
    pub len: usize,
    pub flags: reply::Flags,
    pub lines: BTreeMap<usize, Cell>,
}

/// The line being typed, which the service has not been sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dirty {
    pub line: usize,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PromptKind {
    Find,
    Goto,
    Command,
    /// A path to open as a file of the person's own.
    OpenPath,
}

#[derive(Clone, Debug)]
pub(crate) enum Mode {
    Edit,
    Pager(Pager),
    Prompt(PromptKind, String),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Status {
    pub text: String,
    pub error: bool,
}

pub struct Screen {
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    /// The component and version the service has open.
    pub(crate) place: Option<(String, String)>,
    pub(crate) doc: Doc,
    pub(crate) files: Vec<reply::Entry>,
    pub(crate) line: usize,
    pub(crate) col: usize,
    /// The column Up and Down try to keep.
    pub(crate) goal: usize,
    pub(crate) top: usize,
    pub(crate) dirty: Option<Dirty>,
    pub(crate) mode: Mode,
    pub(crate) status: Status,
    queue: VecDeque<Pending>,
    flight: Option<(u32, Pending)>,
    next_id: u32,
    pub(crate) building: bool,
    polled: Option<u64>,
    /// The edits the service answered `ok`, as commands, for the draft.
    pub(crate) log: Vec<String>,
    log_bytes: usize,
    pub(crate) log_full: bool,
    pub(crate) version: u64,
    /// A line to go to when the next `Meta` answer comes.
    pub(crate) goto: Option<usize>,
    /// A draft being restored: where the cursor and the typed line go once its
    /// edits have replayed.
    pub(crate) resume: Option<Draft>,
    pub(crate) quit: bool,
    /// Whether the file open is one of the person's own, not a component's source:
    /// Ctrl-O then asks for a path, not for the file within the source.
    pub(crate) file_mode: bool,
    /// Ctrl-S was pressed and the draft line sent; the app saves, once.
    pub(crate) save: bool,
    /// The path typed at the open-file prompt, until the app takes it.
    pub(crate) open: Option<String>,
    /// Counts the times the screen changed lines it already shows. An answer
    /// about lines asked for before a change would put the old text back, so
    /// it is dropped.
    pub(crate) epoch: u32,
}

impl Screen {
    /// A screen whose text area is `rows` by `cols` cells, gutter included.
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows: rows.max(1),
            cols: cols.max(8),
            place: None,
            doc: Doc::default(),
            files: Vec::new(),
            line: 1,
            col: 0,
            goal: 0,
            top: 1,
            dirty: None,
            mode: Mode::Edit,
            status: Status::default(),
            queue: VecDeque::new(),
            flight: None,
            next_id: 1,
            building: false,
            polled: None,
            log: Vec::new(),
            log_bytes: 0,
            log_full: false,
            version: 0,
            goto: None,
            resume: None,
            quit: false,
            file_mode: false,
            save: false,
            open: None,
            epoch: 0,
        }
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        self.rows = rows.max(1);
        self.cols = cols.max(8);
        self.follow();
    }

    pub fn wants_quit(&self) -> bool {
        self.quit
    }

    /// Whether the file the screen shows is one of the person's own (not a
    /// component's source): Ctrl-O then asks for another path.
    pub fn set_file_mode(&mut self, on: bool) {
        self.file_mode = on;
    }

    /// Whether Ctrl-S asked for a save since the last look; the draft line was
    /// sent with it, so once its answer is in the file has the text. Told once.
    pub fn take_save(&mut self) -> bool {
        core::mem::take(&mut self.save)
    }

    /// The path typed at the open-file prompt (Ctrl-O, in a file or with nothing
    /// open) since the last look. Told once.
    pub fn take_open(&mut self) -> Option<String> {
        self.open.take()
    }

    /// The file was saved: nothing differs from the disk, and there is nothing to
    /// undo (the session forgets its undo steps when it keeps the text).
    pub fn saved(&mut self) {
        self.doc.flags.changed = false;
        self.doc.flags.undo = false;
    }

    /// A line for the status row, for what the app itself did (a save, a file
    /// that did not open).
    pub fn note(&mut self, text: impl Into<String>, error: bool) {
        self.say(text, error);
    }

    /// The cursor's line (from 1) and column (from 0) in the file on screen.
    pub fn position(&self) -> (usize, usize) {
        (self.line, self.col)
    }

    /// Whether a build is running on the host.
    pub fn is_building(&self) -> bool {
        self.building
    }

    /// Whether a line is being typed that has not gone to the service yet.
    pub fn typing(&self) -> bool {
        self.dirty.is_some()
    }

    /// How many edits the draft holds, those not yet answered included.
    pub fn edits(&self) -> usize {
        self.log.len() + self.unanswered_edits().count()
    }

    /// Opens `component` (its running version by default), with the cursor at
    /// `at` if one is given. Edits not built in a session already open are
    /// never dropped for it; the service says so.
    pub fn open(&mut self, component: &str, label: Option<&str>, at: Option<(&str, u32)>) {
        self.commit();
        self.say(format!("opening {component}..."), false);
        self.push(
            Kind::Open,
            Request::Open {
                component: component.into(),
                label: label.map(String::from),
                at: at.map(|(path, line)| (path.into(), line)),
            },
        );
        self.reload_all();
    }

    /// Brings a draft back: opens its version, replays its edits and puts the
    /// cursor and the typed line where they were.
    pub fn resume(&mut self, draft: Draft) {
        self.say(
            format!("restoring {} {}...", draft.component, draft.label),
            false,
        );
        self.push(
            Kind::Open,
            Request::Open {
                component: draft.component.clone(),
                label: Some(draft.label.clone()),
                at: None,
            },
        );
        for command in &draft.commands {
            self.push(Kind::Replay(command.clone()), Request::Cmd(command.clone()));
        }
        self.resume = Some(draft);
        self.reload_all();
    }

    /// Asks again for the file, its list and the lines on screen.
    pub(crate) fn reload_all(&mut self) {
        self.doc.lines.clear();
        self.epoch = self.epoch.wrapping_add(1);
        self.push(Kind::Meta, Request::Cmd("view 1,1".into()));
        self.push(Kind::Files, Request::Cmd("files".into()));
    }

    pub(crate) fn push(&mut self, kind: Kind, request: Request) {
        let epoch = self.epoch;
        self.queue.push_back(Pending {
            kind,
            request,
            epoch,
        });
    }

    pub(crate) fn say(&mut self, text: impl Into<String>, error: bool) {
        self.status = Status {
            text: text.into(),
            error,
        };
    }

    /// The next request for the service, if none is waiting for an answer.
    pub fn next_request(&mut self) -> Option<Outgoing> {
        if self.flight.is_some() {
            return None;
        }
        let pending = self.queue.pop_front()?;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let request = pending.request.clone();
        self.flight = Some((id, pending));
        Some(Outgoing { id, request })
    }

    /// Whether any request is waiting or unanswered.
    pub fn busy(&self) -> bool {
        self.flight.is_some() || !self.queue.is_empty()
    }

    /// The service could not be reached for request `id`. The screen keeps what
    /// the founder typed and asks for its lines again.
    pub fn failed(&mut self, id: u32, why: &str) {
        let Some(pending) = self.take(id) else { return };
        self.say(format!("the edit service did not answer: {why}"), true);
        self.queue.retain(|p| !matches!(p.kind, Kind::Replay(_)));
        if matches!(pending.kind, Kind::Edit(_) | Kind::Reload(_)) {
            self.reload_all();
        }
    }

    fn take(&mut self, id: u32) -> Option<Pending> {
        match self.flight.take() {
            Some((got, pending)) if got == id => Some(pending),
            other => {
                self.flight = other;
                None
            }
        }
    }

    /// Time passes; a running build is looked at every [`POLL_MS`].
    pub fn tick(&mut self, now_ms: u64) {
        let due = self
            .polled
            .is_none_or(|at| now_ms.saturating_sub(at) >= POLL_MS);
        let waiting = self.queue.iter().any(|p| p.kind == Kind::Status);
        if self.building && due && !waiting {
            self.polled = Some(now_ms);
            self.push(Kind::Status, Request::Cmd("status".into()));
        }
    }

    /// Remembers an edit the service answered `ok`, for the draft.
    pub(crate) fn keep(&mut self, command: &str) {
        self.version += 1;
        if !draft::replays(command) {
            return;
        }
        if self.log.len() >= draft::MAX_COMMANDS
            || self.log_bytes + command.len() + 8 > draft::MAX_DRAFT / 2
        {
            self.log_full = true;
            return;
        }
        self.log_bytes += command.len() + 8;
        self.log.push(command.into());
    }

    /// What to keep so a crash loses nothing: None before a version is open.
    pub fn draft(&self) -> Option<Draft> {
        let (component, label) = self.place.clone()?;
        Some(Draft {
            component,
            label,
            file: self.doc.path.clone(),
            line: self.line,
            col: self.col,
            top: self.top,
            commands: self
                .log
                .iter()
                .cloned()
                .chain(self.unanswered_edits())
                .collect(),
            dirty: self.dirty.as_ref().map(|d| (d.line, d.text.clone())),
        })
    }

    /// Edits sent or waiting to be sent: the draft keeps them, since a crash
    /// before the answer would otherwise lose what the founder typed.
    fn unanswered_edits(&self) -> impl Iterator<Item = String> + '_ {
        let pending = self.flight.iter().map(|(_, p)| p).chain(&self.queue);
        pending.filter_map(|p| match &p.kind {
            Kind::Edit(command) if draft::replays(command) => Some(command.clone()),
            _ => None,
        })
    }

    /// Changes whenever [`Screen::draft`] would differ; the caller saves when it
    /// moves.
    pub fn draft_version(&self) -> u64 {
        self.version
    }

    /// Whether the draft holds all of the founder's edits; a full one keeps the
    /// first ones, and the screen says so.
    pub fn draft_full(&self) -> bool {
        self.log_full
    }
}
