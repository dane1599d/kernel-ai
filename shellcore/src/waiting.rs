//! What the apps ask while the agent waits on the model (the founder's test 2,
//! item 7). A model call takes 3 to 70 s, and the shell answered nothing else
//! meanwhile: Files sat on "Loading…", and the Editor and Settings waited.
//! Now a host that can look at them while the model's call is in flight
//! ([`Host::act_serving`]) hands the apps' waiting requests here, and those
//! that only read are answered at once: an effect batch whose every call is a
//! read-only row, the editor's read of a file, and Settings' reads. Anything
//! that changes something waits for the line to end, as before: the host
//! gives it back to `desktop` to keep (#890), so a shell that dies mid-line
//! loses none, and marks it in [`Held`], which says what must wait behind it
//! and what the host takes again after the line.
//!
//! [`Host::act_serving`]: crate::Host::act_serving

use crate::Shell;
use crate::effects::{self, Effect, EffectDone};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

/// What kind of request a held one is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Effects,
    File,
    Settings,
    /// What came of a request, still to be given to its app: the person's
    /// Stop withdrew the call that gave it before `desktop` took it.
    Answer,
}

/// One held request: its kind, its app, and the host's own.
struct Item<T> {
    kind: Kind,
    app: String,
    request: T,
}

/// What the apps asked while the model thought that was not a read, in the
/// order it came, and the one running now, if one is: it still holds back
/// what came after it until it is done.
/// - A request of an app with anything held waits behind it, so a read never
///   overtakes a change of the same app.
/// - While a Settings change is held, every request waits, so none is answered
///   under what the change takes away. The first Settings request held is a
///   change, as one that only reads is answered.
///
/// The host takes nothing of an app held back ([`Held::apps`]), so each app
/// has at most one request held, and an answer still to give.
pub struct Held<T> {
    items: VecDeque<Item<T>>,
    running: Option<(Kind, String)>,
}

impl<T> Default for Held<T> {
    fn default() -> Self {
        Self {
            items: VecDeque::new(),
            running: None,
        }
    }
}

impl<T> Held<T> {
    fn all(&self) -> impl Iterator<Item = (Kind, &str)> {
        self.running
            .iter()
            .map(|(kind, app)| (*kind, app.as_str()))
            .chain(self.items.iter().map(|i| (i.kind, i.app.as_str())))
    }

    /// Whether nothing is held or running.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.running.is_none()
    }

    /// Whether a request of `app` must wait behind what is held.
    pub fn blocks(&self, app: &str) -> bool {
        self.blocks_all() || self.all().any(|(_, held)| held == app)
    }

    /// Whether every request must wait: a Settings change is held or runs.
    pub fn blocks_all(&self) -> bool {
        self.all().any(|(kind, _)| kind == Kind::Settings)
    }

    /// The apps held back, each once, for a take to pass by.
    pub fn apps(&self) -> Vec<String> {
        let mut apps: Vec<String> = Vec::new();
        for (_, app) in self.all() {
            if !apps.iter().any(|a| a == app) {
                apps.push(String::from(app));
            }
        }
        apps
    }

    /// Holds `request`, of `kind`, from `app`, behind what is held already.
    pub fn hold(&mut self, kind: Kind, app: &str, request: T) {
        self.items.push_back(Item {
            kind,
            app: String::from(app),
            request,
        });
    }

    /// The oldest held request, to run now: it holds back what came after it
    /// until [`Held::done`].
    pub fn start(&mut self) -> Option<T> {
        let item = self.items.pop_front()?;
        self.running = Some((item.kind, item.app));
        Some(item.request)
    }

    /// A request of `app` the host runs now from elsewhere (the main loop's
    /// own effect batch): it holds back what comes after it, as one from
    /// [`Held::start`] does, until [`Held::done`].
    pub fn running(&mut self, kind: Kind, app: &str) {
        self.running = Some((kind, String::from(app)));
    }

    /// The request [`Held::start`] gave, or [`Held::running`] named, has run.
    pub fn done(&mut self) {
        self.running = None;
    }
}

/// An app's request that came while a model call was in flight.
#[derive(Clone, Copy, Debug)]
pub enum Waiting<'a> {
    /// An app's effect lines, as [`Shell::run_effects_from`] takes them.
    Effects {
        app: &'a str,
        lines: &'a str,
        user: bool,
    },
    /// The editor's read of a whole file, as [`Shell::door_read`] takes it.
    FileRead { app: &'a str, path: &'a str },
    /// The Settings app's requests, as [`Shell::settings`] takes each: its
    /// text, and whether a person's key or click sent it.
    Settings {
        app: &'a str,
        requests: &'a [(&'a str, bool)],
    },
}

/// What a request that only reads came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Served {
    Effects(Vec<EffectDone>),
    FileRead(Result<Vec<u8>, String>),
    /// One answer for each request, in order.
    Settings(Vec<Result<String, String>>),
}

impl Shell {
    /// Answers `waiting` if it only reads; `None` for one that would change
    /// anything, which waits for the line. Called while the line's model call
    /// is in flight: nothing of the line has run yet, and the line's own run
    /// sets what a read sets (its output, its value) after it.
    pub fn serve_waiting(&mut self, waiting: Waiting<'_>) -> Option<Served> {
        // What the person's line is in the middle of is the line's, never a
        // read's: a read neither sees nor leaves a question, a pending call,
        // an undo waiting for a yes, the menu's ONCE or the panel's marks.
        let line = LineState::take(self);
        let served = match waiting {
            Waiting::Effects { app, lines, user } => self
                .reads_only(lines)
                .then(|| Served::Effects(self.run_effects_from(app, lines, user))),
            Waiting::FileRead { app, path } => Some(Served::FileRead(self.door_read(app, path))),
            Waiting::Settings { app, requests } => {
                match requests
                    .iter()
                    .any(|(text, _)| crate::settings::is_change(text))
                {
                    true => None,
                    false => Some(Served::Settings(
                        requests
                            .iter()
                            .map(|(text, user)| self.settings(app, text, *user))
                            .collect(),
                    )),
                }
            }
        };
        line.put_back(self);
        served
    }

    /// Whether an effect batch only reads: notes, and calls to rows, the OS's
    /// or an app's, that are read-only. A name no row has, a line that does
    /// not parse, and a Terminal's `shell_line` all wait for the line, as
    /// every effect did before.
    fn reads_only(&self, lines: &str) -> bool {
        let apps = self.host.apps().map(|apps| apps.borrow());
        lines
            .lines()
            .filter_map(effects::parse_line)
            .all(|(_, effect)| match effect {
                Ok(Effect::Notify(_)) => true,
                Ok(Effect::Call { name, .. }) if name == crate::appline::SHELL_LINE => false,
                Ok(Effect::Call { name, .. }) => match agentabi::find(&agentabi::ROWS, &name) {
                    Some(row) => row.facts.read_only,
                    None => apps
                        .as_ref()
                        .and_then(|apps| apps.rows.find(&name))
                        .is_some_and(|row| row.row.facts.read_only),
                },
                Err(_) => false,
            })
    }
}

/// The person's line's own state, kept apart while a read is served.
struct LineState {
    owner_once: Option<u64>,
    pending: Option<crate::actions::Pending>,
    undo_asking: Option<u64>,
    clarifying: Option<String>,
    asked_model: bool,
    worded: bool,
}

impl LineState {
    fn take(shell: &mut Shell) -> Self {
        Self {
            owner_once: shell.owner_once.take(),
            pending: shell.pending.take(),
            undo_asking: shell.undo_asking.take(),
            clarifying: shell.clarifying.take(),
            asked_model: core::mem::take(&mut shell.asked_model),
            worded: core::mem::take(&mut shell.worded),
        }
    }

    /// Puts the line's state back, over anything a read set. A read's own
    /// pending call, if it left one, is dropped: the rows a read runs never
    /// ask, so they never leave one.
    fn put_back(self, shell: &mut Shell) {
        shell.owner_once = self.owner_once;
        shell.pending = self.pending;
        shell.undo_asking = self.undo_asking;
        shell.clarifying = self.clarifying;
        shell.asked_model = self.asked_model;
        shell.worded = self.worded;
    }
}
